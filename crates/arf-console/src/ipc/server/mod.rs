//! IPC server that listens on a Unix socket (or named pipe on Windows).
//!
//! Runs in a dedicated thread with a tokio current_thread runtime.
//! Each connection is handled as a simple HTTP-like JSON-RPC endpoint:
//! read one request, dispatch via mpsc channel, await oneshot reply, respond.
//! A client that opens with a newline-terminated raw `attach` request instead
//! keeps the connection and receives events (see `attached`).
//!
//! Each server run owns an event bus whose dispatcher runs on the server's
//! runtime. The bus is also published as the process-wide current bus for
//! the lifetime of the run.

mod attached;
mod connection;
mod dispatch;
mod socket_path;
mod transport;

use crate::ipc::events::{EventBus, Limits, clear_current_bus, install_current_bus};
use crate::ipc::protocol::IpcRequest;
use crate::ipc::session::{SessionInfo, SessionType, remove_session, write_session};
use crate::sync::LockRecover;
use connection::ConnectionContext;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

/// Global shutdown token and join handle for the server thread.
static SERVER_HANDLE: OnceLock<Mutex<Option<ServerState>>> = OnceLock::new();
static SERVER_ATEXIT_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Return whether the IPC server is currently running.
pub fn is_server_running() -> bool {
    let handle_store = match SERVER_HANDLE.get() {
        Some(handle_store) => handle_store,
        None => return false,
    };
    handle_store
        .lock_recover()
        .as_ref()
        .is_some_and(|state| !state.join_handle.is_finished())
}

struct ServerState {
    cancel_token: CancellationToken,
    join_handle: std::thread::JoinHandle<()>,
    /// Socket path (used on Unix for cleanup; on Windows, named pipes are
    /// cleaned up automatically when the server is dropped).
    #[cfg_attr(windows, allow(dead_code))]
    socket_path: String,
    /// Whether the socket directory was auto-created (not a custom `--ipc-bind`
    /// path).  Only auto-created directories are cleaned up on shutdown.
    #[cfg_attr(windows, allow(dead_code))]
    auto_created_dir: bool,
}

/// Start the IPC server in a background thread.
///
/// Returns the [`SessionInfo`] on success (includes the socket path).
/// `r_version` is recorded in the session file as given.
/// Returns an error if the server is already running.
#[allow(clippy::too_many_arguments)]
pub fn start_server(
    tx: mpsc::Sender<IpcRequest>,
    bind: Option<&str>,
    started_at: &str,
    r_home: Option<String>,
    r_version: Option<String>,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    session_type: SessionType,
) -> std::io::Result<SessionInfo> {
    if !SERVER_ATEXIT_REGISTERED.swap(true, Ordering::AcqRel) {
        let result = unsafe { libc::atexit(stop_server_at_exit) };
        if result != 0 {
            SERVER_ATEXIT_REGISTERED.store(false, Ordering::Release);
            log::warn!("Failed to register IPC server cleanup with atexit");
        }
    }
    // Acquire the lock once and hold it through check-and-set to avoid TOCTOU.
    let handle_store = SERVER_HANDLE.get_or_init(|| Mutex::new(None));
    let mut guard = handle_store.lock().unwrap();

    if guard.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "IPC server is already running",
        ));
    }

    let pid = std::process::id();
    let (socket_path, dir_created) = match bind {
        Some(path) => (path.to_string(), false),
        None => socket_path::get_socket_path(pid).ok_or_else(|| {
            std::io::Error::other(format!(
                "Failed to determine a safe IPC socket path for pid {pid}. \
                 All candidate directories were unsafe or could not be created. \
                 Check the log for details."
            ))
        })?,
    };

    // Remove stale socket file if it exists.
    #[cfg(unix)]
    socket_path::probe_stale_socket(&socket_path, bind.is_some())?;

    let path = socket_path.clone();
    let started_at_owned = started_at.to_string();
    let r_home_clone = r_home.clone();
    let log_file_clone = log_file.clone();
    let history_session_id_clone = history_session_id;
    let session_type_clone = session_type;
    let cancel_token = CancellationToken::new();
    let token_clone = cancel_token.clone();

    // Channel for the server thread to confirm successful bind before we
    // write the session file.
    let (bind_tx, bind_rx) = std::sync::mpsc::sync_channel::<Result<(), std::io::Error>>(1);

    let join_handle = std::thread::Builder::new()
        .name("arf-ipc-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for IPC server");

            rt.block_on(async move {
                // The bus lives exactly as long as this run of the server. It
                // is created before the bind is confirmed, so it is in place
                // before any client can connect.
                let (bus, dispatcher) = EventBus::new(Limits::default());
                let dispatcher_task = tokio::spawn(dispatcher.run());
                let generation = install_current_bus(bus.clone());
                let ctx = ConnectionContext::new(tx, bus.clone(), token_clone);

                if let Err(e) = transport::run_server(
                    &path,
                    &started_at_owned,
                    r_home_clone,
                    log_file_clone,
                    history_session_id_clone,
                    session_type_clone,
                    ctx,
                    bind_tx,
                )
                .await
                {
                    log::error!("IPC server error: {}", e);
                }

                // Only remove our own bus: a server started after this one
                // may already have installed another.
                clear_current_bus(generation);
                bus.close();
                let _ = dispatcher_task.await;
            });
        })?;

    // Wait for the server thread to confirm that bind succeeded.
    match bind_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = join_handle.join();
            return Err(e);
        }
        Err(_) => {
            let _ = join_handle.join();
            return Err(std::io::Error::other("IPC server thread failed to start"));
        }
    }

    // Store handle for later shutdown (lock is still held — no TOCTOU)
    *guard = Some(ServerState {
        cancel_token,
        join_handle,
        socket_path: socket_path.clone(),
        auto_created_dir: dir_created,
    });

    // Note: session metadata is cached in the server thread right before
    // bind confirmation, so it is available before any connection is served.

    // Write session metadata
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let session = SessionInfo {
        pid,
        socket_path: socket_path.clone(),
        r_version,
        r_home,
        cwd,
        started_at: started_at.to_string(),
        session_type,
        log_file,
        history_session_id,
    };

    if let Err(e) = write_session(&session) {
        log::warn!("Failed to write session file: {}", e);
    }

    Ok(session)
}

extern "C" fn stop_server_at_exit() {
    stop_server();
}

/// Stop the IPC server gracefully.
pub fn stop_server() {
    let handle_store = match SERVER_HANDLE.get() {
        Some(h) => h,
        None => return,
    };

    let state = handle_store.lock().unwrap().take();
    if let Some(state) = state {
        // Signal the server to stop; in-flight connection handlers will be
        // dropped when the tokio runtime shuts down (acceptable for local IPC).
        log::debug!("Shutting down IPC server, in-flight connections will be dropped");
        state.cancel_token.cancel();

        // Remove the socket pathname to prevent new clients from connecting
        // during shutdown.  For auto-created directories (not custom --ipc-bind),
        // also remove the parent directory if it is now empty.
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&state.socket_path);
            if state.auto_created_dir
                && let Some(parent) = std::path::Path::new(&state.socket_path).parent()
            {
                // remove_dir only succeeds if the directory is empty,
                // which is the desired behavior — we must not remove
                // XDG_RUNTIME_DIR/arf/ if other arf processes have
                // sockets there.
                let _ = std::fs::remove_dir(parent);
            }
        }

        // Wait for the server thread to finish
        let _ = state.join_handle.join();

        // Remove session metadata
        remove_session(std::process::id());
    }
}
