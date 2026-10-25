//! Listener setup and accept loops for the Unix socket / Windows named pipe.

use super::SocketIdentity;
use super::connection::{ConnectionContext, handle_connection};
use crate::ipc::session::SessionType;
use crate::ipc::session_result::set_session_meta;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serve one accepted connection on its own task.
fn spawn_connection<S>(stream: S, ctx: ConnectionContext)
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = handle_connection(stream, ctx).await {
            log::debug!("IPC connection error: {}", e);
        }
    });
}

/// Run the actual server loop.
#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_server(
    socket_path: &str,
    started_at: &str,
    r_home: Option<String>,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    session_type: SessionType,
    ctx: ConnectionContext,
    bind_tx: std::sync::mpsc::SyncSender<Result<Option<SocketIdentity>, std::io::Error>>,
) -> std::io::Result<()> {
    let listener = match tokio::net::UnixListener::bind(socket_path) {
        Ok(listener) => listener,
        Err(e) => {
            let _ = bind_tx.send(Err(std::io::Error::new(e.kind(), e.to_string())));
            return Err(e);
        }
    };
    let identity = match SocketIdentity::from_path(socket_path) {
        Ok(identity) => identity,
        Err(error) => {
            let _ = bind_tx.send(Err(std::io::Error::new(error.kind(), error.to_string())));
            return Err(error);
        }
    };
    // Locals drop in reverse declaration order: unlink before the listener
    // closes, including when this future is dropped or unwinds.
    let _socket_path_guard = super::SocketPathGuard::new(socket_path, identity);

    // Restrict socket permissions so only the owner can connect.
    // The default PID-based path lives under a 0700 sessions dir,
    // but custom --ipc-bind paths inherit the parent dir's umask.
    // Use fd-based fchmod to avoid TOCTOU symlink race.
    //
    // NOTE: There is a brief race window between bind() and fchmod()
    // where the socket exists with umask-inherited permissions. For
    // custom --ipc-bind paths in shared directories, operators should
    // ensure the parent directory is restricted (e.g. 0700).
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::fchmod(listener.as_raw_fd(), 0o600) };
        if ret != 0 {
            log::warn!(
                "Could not set socket permissions on {}: {}",
                socket_path,
                std::io::Error::last_os_error()
            );
        }
    }
    // Cache session metadata BEFORE signalling bind success, so it
    // is guaranteed to be available when the first request arrives.
    set_session_meta(
        socket_path.to_string(),
        started_at.to_string(),
        r_home,
        log_file,
        history_session_id,
        session_type,
    );
    let _ = bind_tx.send(Ok(Some(identity)));
    log::info!("IPC server listening on {}", socket_path);

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        spawn_connection(stream, ctx.clone());
                    }
                    Err(e) => {
                        if ctx.cancel.is_cancelled() {
                            break;
                        }
                        log::warn!("IPC accept error: {}", e);
                    }
                }
            }
            _ = ctx.cancel.cancelled() => {
                log::info!("IPC server shutting down");
                break;
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_server(
    socket_path: &str,
    started_at: &str,
    r_home: Option<String>,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    session_type: SessionType,
    ctx: ConnectionContext,
    bind_tx: std::sync::mpsc::SyncSender<Result<Option<SocketIdentity>, std::io::Error>>,
) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    // Create the first pipe instance
    let mut server = match ServerOptions::new()
        .first_pipe_instance(true)
        .create(socket_path)
    {
        Ok(s) => {
            // Cache session metadata BEFORE signalling bind success, so it
            // is guaranteed to be available when the first request arrives.
            set_session_meta(
                socket_path.to_string(),
                started_at.to_string(),
                r_home,
                log_file,
                history_session_id,
                session_type,
            );
            let _ = bind_tx.send(Ok(None));
            s
        }
        Err(e) => {
            let _ = bind_tx.send(Err(std::io::Error::new(e.kind(), e.to_string())));
            return Err(e);
        }
    };
    log::info!("IPC server listening on {}", socket_path);

    loop {
        tokio::select! {
            result = server.connect() => {
                match result {
                    Ok(()) => {
                        let connected = server;

                        // Create a new pipe instance for the next connection
                        server = ServerOptions::new().create(socket_path)?;

                        spawn_connection(connected, ctx.clone());
                    }
                    Err(e) => {
                        if ctx.cancel.is_cancelled() {
                            break;
                        }
                        log::warn!("IPC accept error: {}", e);
                    }
                }
            }
            _ = ctx.cancel.cancelled() => {
                log::info!("IPC server shutting down");
                break;
            }
        }
    }
    Ok(())
}
