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

#[cfg(test)]
pub(in crate::ipc) use attached::EventNotification;

use crate::ipc::events::{EventBus, Limits};
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
    #[cfg(unix)]
    socket_identity: Option<SocketIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SocketIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[cfg(unix)]
impl SocketIdentity {
    fn from_path(path: &str) -> std::io::Result<Self> {
        use std::os::unix::fs::FileTypeExt;
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::other(
                "IPC socket path does not refer to a Unix socket",
            ));
        }
        Ok(Self::from_metadata(&metadata))
    }

    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[cfg(unix)]
fn remove_owned_socket(path: &str, expected: Option<SocketIdentity>) {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let Some(expected) = expected else {
        return;
    };
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    // Preserve paths already replaced since the identity was captured. A
    // replacement between this check and unlink can still race; POSIX has
    // no conditional unlink.
    if metadata.file_type().is_socket()
        && metadata.dev() == expected.device
        && metadata.ino() == expected.inode
    {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
pub(super) struct SocketPathGuard {
    path: String,
    identity: SocketIdentity,
}

#[cfg(unix)]
impl SocketPathGuard {
    pub(super) fn new(path: &str, identity: SocketIdentity) -> Self {
        Self {
            path: path.to_string(),
            identity,
        }
    }
}

#[cfg(unix)]
impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        remove_owned_socket(&self.path, Some(self.identity));
    }
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
    let (bind_tx, bind_rx) =
        std::sync::mpsc::sync_channel::<Result<Option<SocketIdentity>, std::io::Error>>(1);

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
                let generation = crate::ipc::install_console_output_bus(bus.clone());
                let ctx = ConnectionContext::new(tx, bus.clone(), token_clone);
                let timer_cancel = ctx.cancel.clone();
                let output_timer = tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
                    interval.tick().await;
                    loop {
                        tokio::select! {
                            _ = timer_cancel.cancelled() => break,
                            _ = interval.tick() => crate::ipc::flush_console_output(),
                        }
                    }
                });

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

                output_timer.abort();
                let _ = output_timer.await;
                // Only remove our own bus: a server started after this one
                // may already have installed another.
                crate::ipc::clear_console_output_bus(generation);
                bus.close();
                let _ = dispatcher_task.await;
            });
        })?;

    // Wait for the server thread to confirm that bind succeeded.
    let _socket_identity = match bind_rx.recv() {
        Ok(Ok(identity)) => identity,
        Ok(Err(e)) => {
            let _ = join_handle.join();
            return Err(e);
        }
        Err(_) => {
            let _ = join_handle.join();
            return Err(std::io::Error::other("IPC server thread failed to start"));
        }
    };

    // Store handle for later shutdown (lock is still held — no TOCTOU)
    *guard = Some(ServerState {
        cancel_token,
        join_handle,
        socket_path: socket_path.clone(),
        auto_created_dir: dir_created,
        #[cfg(unix)]
        socket_identity: _socket_identity,
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
        // Unlink before cancellation, keeping the old listener alive through
        // cleanup. A later server may bind the freed path; the identity check
        // in the old listener's guard preserves that new socket.
        // For auto-created directories (not custom --ipc-bind), also remove
        // the parent directory if it is now empty.
        #[cfg(unix)]
        {
            remove_owned_socket(&state.socket_path, state.socket_identity);
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

        // Signal the server to stop only after unlinking. In-flight connection
        // handlers are dropped when the tokio runtime shuts down.
        log::debug!("Shutting down IPC server, in-flight connections will be dropped");
        state.cancel_token.cancel();

        // Wait for the server thread to finish
        let _ = state.join_handle.join();

        // Remove session metadata
        remove_session(std::process::id());
    }
}

#[cfg(test)]
mod ownership_tests {
    #[cfg(unix)]
    #[test]
    fn cleanup_preserves_replacement_at_socket_path() {
        use super::{SocketIdentity, remove_owned_socket};
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let identity = SocketIdentity::from_path(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "replacement sentinel").unwrap();

        remove_owned_socket(path.to_str().unwrap(), Some(identity));

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "replacement sentinel"
        );
    }

    #[cfg(unix)]
    #[test]
    fn socket_path_guard_removes_its_socket_when_dropped() {
        use super::{SocketIdentity, SocketPathGuard};
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let identity = SocketIdentity::from_path(path.to_str().unwrap()).unwrap();
        let guard = SocketPathGuard::new(path.to_str().unwrap(), identity);
        assert_eq!(
            super::socket_path::probe_stale_socket(path.to_str().unwrap(), true)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AlreadyExists
        );
        drop(guard);
        assert!(!path.exists(), "owned socket remained after guard drop");
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn socket_path_guard_preserves_replacement_after_prior_cleanup() {
        use super::{SocketIdentity, SocketPathGuard, remove_owned_socket};
        use std::os::unix::net::{UnixListener, UnixStream};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let identity = SocketIdentity::from_path(path.to_str().unwrap()).unwrap();
        let guard = SocketPathGuard::new(path.to_str().unwrap(), identity);

        remove_owned_socket(path.to_str().unwrap(), Some(identity));
        let _replacement = UnixListener::bind(&path).unwrap();
        drop(guard);

        UnixStream::connect(&path).expect("guard removed the replacement socket");
    }
}
