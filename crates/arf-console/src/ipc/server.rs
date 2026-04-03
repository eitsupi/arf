//! IPC server that listens on a Unix socket (or named pipe on Windows).
//!
//! Runs in a dedicated thread with a tokio current_thread runtime.
//! Each connection is handled as a simple HTTP-like JSON-RPC endpoint:
//! read one request, dispatch via mpsc channel, await oneshot reply, respond.

use crate::ipc::protocol::{
    EvaluateParams, HistoryParams, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, IpcMethod,
    IpcRequest, IpcResponse, JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND, PARSE_ERROR,
    ShutdownResult, UserInputParams,
};
use crate::ipc::session::{SessionInfo, remove_session, write_session};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// Global shutdown token and join handle for the server thread.
static SERVER_HANDLE: OnceLock<Mutex<Option<ServerState>>> = OnceLock::new();

struct ServerState {
    cancel_token: CancellationToken,
    join_handle: std::thread::JoinHandle<()>,
    /// Socket path (used on Unix for cleanup; on Windows, named pipes are
    /// cleaned up automatically when the server is dropped).
    #[cfg_attr(windows, allow(dead_code))]
    socket_path: String,
}

/// Start the IPC server in a background thread.
///
/// Returns the [`SessionInfo`] on success (includes the socket path).
/// Returns an error if the server is already running.
pub fn start_server(
    tx: mpsc::Sender<IpcRequest>,
    bind: Option<&str>,
    started_at: &str,
    log_file: Option<String>,
    history_session_id: Option<i64>,
) -> std::io::Result<SessionInfo> {
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
    let socket_path = match bind {
        Some(path) => path.to_string(),
        None => get_socket_path(pid).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Cannot find a safe directory for the IPC socket. \
                 Check the log for details.",
            )
        })?,
    };

    // Remove stale socket file if it exists. When a custom --bind path is
    // used, only remove the path if it is actually a Unix socket to avoid
    // accidentally deleting unrelated files. For sockets, attempt a connect
    // to distinguish stale from active: if connect succeeds, another process
    // is listening and we must not take over.
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixStream;
        match std::fs::symlink_metadata(&socket_path) {
            Ok(meta) if meta.file_type().is_socket() => {
                if bind.is_some() {
                    // Custom bind path: verify the socket is stale before removing
                    match UnixStream::connect(&socket_path) {
                        Ok(_) => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                format!("IPC socket already in use at path: {}", socket_path),
                            ));
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::ConnectionRefused
                                || e.kind() == std::io::ErrorKind::NotFound =>
                        {
                            // ConnectionRefused: no listener (stale socket).
                            // NotFound: socket disappeared between metadata
                            // check and connect (race); safe to proceed.
                            let _ = std::fs::remove_file(&socket_path);
                        }
                        Err(e) => {
                            return Err(std::io::Error::new(
                                e.kind(),
                                format!("Cannot probe socket at {}: {}", socket_path, e),
                            ));
                        }
                    }
                } else {
                    // Default PID-based path — safe to remove (same PID reuse)
                    let _ = std::fs::remove_file(&socket_path);
                }
            }
            Ok(_) if bind.is_some() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "bind path already exists and is not a socket: {}",
                        socket_path
                    ),
                ));
            }
            Ok(_) => {
                // Default path (PID-based) — safe to remove
                let _ = std::fs::remove_file(&socket_path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // Does not exist
            Err(e) => return Err(e), // Propagate unexpected errors (e.g. EACCES)
        }
    }

    let path = socket_path.clone();
    let started_at_owned = started_at.to_string();
    let log_file_clone = log_file.clone();
    let history_session_id_clone = history_session_id;
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
                if let Err(e) = run_server(
                    &path,
                    &started_at_owned,
                    log_file_clone,
                    history_session_id_clone,
                    tx,
                    token_clone,
                    bind_tx,
                )
                .await
                {
                    log::error!("IPC server error: {}", e);
                }
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
    });

    // Note: session metadata is cached in the server thread right before
    // bind confirmation, so it is available before any connection is served.

    // Write session metadata
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let r_version = {
        let tmpfile = tempfile::Builder::new()
            .prefix(".arf_ipc_rver_")
            .suffix(".txt")
            .tempfile()
            .ok();
        if let Some(ref tmpfile) = tmpfile {
            let tmppath = tmpfile.path().display().to_string().replace('\\', "/");
            let code = format!(
                r#"writeLines(paste0(R.version$major, ".", R.version$minor), "{tmppath}")"#
            );
            let _ = arf_harp::eval_string(&code);
            std::fs::read_to_string(tmpfile.path())
                .ok()
                .map(|s| s.trim().to_string())
        } else {
            None
        }
    };

    let session = SessionInfo {
        pid,
        socket_path: socket_path.clone(),
        r_version,
        cwd,
        started_at: started_at.to_string(),
        log_file,
        history_session_id,
    };

    if let Err(e) = write_session(&session) {
        log::warn!("Failed to write session file: {}", e);
    }

    Ok(session)
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

        // Remove socket file so accept() fails (unblocks the loop)
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&state.socket_path);
        }

        // Wait for the server thread to finish
        let _ = state.join_handle.join();

        // Remove session metadata
        remove_session(std::process::id());
    }
}

/// Get the socket/pipe path for a given PID.
///
/// On Unix, uses `$XDG_RUNTIME_DIR/arf/<pid>.sock` (the XDG-correct location
/// for runtime sockets).  Falls back to `<temp_dir>/arf-<uid>/<pid>.sock`
/// when `XDG_RUNTIME_DIR` is not set (e.g. macOS, non-systemd Linux).
///
/// The socket directory is validated for safety (not a symlink, owned by
/// the current user, not writable by group/other).  If validation fails,
/// a per-process fallback directory is used instead.
fn get_socket_path(pid: u32) -> Option<String> {
    #[cfg(unix)]
    {
        let primary = dirs::runtime_dir()
            .map(|d| d.join("arf"))
            .unwrap_or_else(|| {
                let uid = unsafe { libc::getuid() };
                std::env::temp_dir().join(format!("arf-{uid}"))
            });
        let fallback = std::env::temp_dir().join(format!("arf-{pid}"));
        select_socket_dir(pid, &[primary, fallback])
    }
    #[cfg(windows)]
    {
        Some(format!(r"\\.\pipe\arf-ipc-{pid}"))
    }
}

/// Validate that a directory is safe to use for an IPC socket: not a
/// symlink, is a directory, owned by the current user, and accessible only
/// by the owner (mode `0700`).  Returns `true` if the path does not exist
/// yet (it will be created securely by the caller).
#[cfg(unix)]
fn is_dir_safe(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    match dir.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            log::warn!("Cannot stat socket directory {}: {e}", dir.display());
            false
        }
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                log::warn!(
                    "Socket directory {} is a symlink — refusing to use it",
                    dir.display()
                );
                return false;
            }
            if !meta.is_dir() {
                log::warn!(
                    "Socket directory path {} exists but is not a directory",
                    dir.display()
                );
                return false;
            }
            if meta.uid() != unsafe { libc::getuid() } {
                log::warn!(
                    "Socket directory {} is not owned by the current user",
                    dir.display()
                );
                return false;
            }
            if meta.permissions().mode() & 0o77 != 0 {
                log::warn!(
                    "Socket directory {} has insecure permissions ({:o})",
                    dir.display(),
                    meta.permissions().mode()
                );
                return false;
            }
            true
        }
    }
}

/// Try each candidate directory in order, returning the socket path for
/// the first one that passes safety validation.  Creates the chosen
/// directory with mode `0700` if it does not exist.
#[cfg(unix)]
fn select_socket_dir(pid: u32, candidates: &[std::path::PathBuf]) -> Option<String> {
    use std::os::unix::fs::DirBuilderExt;

    for dir in candidates {
        if is_dir_safe(dir) {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            if let Err(e) = builder.create(dir) {
                log::warn!("Failed to create directory {}: {e}", dir.display());
                continue;
            }
            return Some(dir.join(format!("{pid}.sock")).display().to_string());
        }
    }

    let dirs: Vec<_> = candidates.iter().map(|d| d.display().to_string()).collect();
    log::error!(
        "All socket directory candidates are unsafe: {}. \
         Refusing to start IPC server.",
        dirs.join(", ")
    );
    None
}

/// Run the actual server loop.
#[cfg(unix)]
async fn run_server(
    socket_path: &str,
    started_at: &str,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    tx: mpsc::Sender<IpcRequest>,
    cancel: CancellationToken,
    bind_tx: std::sync::mpsc::SyncSender<Result<(), std::io::Error>>,
) -> std::io::Result<()> {
    let listener = match tokio::net::UnixListener::bind(socket_path) {
        Ok(l) => {
            // Restrict socket permissions so only the owner can connect.
            // The default PID-based path lives under a 0700 sessions dir,
            // but custom --bind paths inherit the parent dir's umask.
            // Use fd-based fchmod to avoid TOCTOU symlink race.
            //
            // NOTE: There is a brief race window between bind() and fchmod()
            // where the socket exists with umask-inherited permissions. For
            // custom --bind paths in shared directories, operators should
            // ensure the parent directory is restricted (e.g. 0700).
            {
                use std::os::unix::io::AsRawFd;
                let ret = unsafe { libc::fchmod(l.as_raw_fd(), 0o600) };
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
            super::set_session_meta(
                socket_path.to_string(),
                started_at.to_string(),
                log_file,
                history_session_id,
            );
            let _ = bind_tx.send(Ok(()));
            l
        }
        Err(e) => {
            let _ = bind_tx.send(Err(std::io::Error::new(e.kind(), e.to_string())));
            return Err(e);
        }
    };
    log::info!("IPC server listening on {}", socket_path);

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, tx).await {
                                log::debug!("IPC connection error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        log::warn!("IPC accept error: {}", e);
                    }
                }
            }
            _ = cancel.cancelled() => {
                log::info!("IPC server shutting down");
                break;
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
async fn run_server(
    socket_path: &str,
    started_at: &str,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    tx: mpsc::Sender<IpcRequest>,
    cancel: CancellationToken,
    bind_tx: std::sync::mpsc::SyncSender<Result<(), std::io::Error>>,
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
            super::set_session_meta(
                socket_path.to_string(),
                started_at.to_string(),
                log_file,
                history_session_id,
            );
            let _ = bind_tx.send(Ok(()));
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
                        let tx = tx.clone();
                        let connected = server;

                        // Create a new pipe instance for the next connection
                        server = ServerOptions::new().create(socket_path)?;

                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(connected, tx).await {
                                log::debug!("IPC connection error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        log::warn!("IPC accept error: {}", e);
                    }
                }
            }
            _ = cancel.cancelled() => {
                log::info!("IPC server shutting down");
                break;
            }
        }
    }
    Ok(())
}

/// Handle a single connection: read request, dispatch, respond.
async fn handle_connection<S>(mut stream: S, tx: mpsc::Sender<IpcRequest>) -> std::io::Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    // Read the full request (up to 1MB).
    //
    // Two strategies:
    // 1. HTTP request with Content-Length: read headers, then read exactly body_len bytes.
    // 2. Raw JSON: read until the buffer parses as valid JSON, or EOF.
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];

    loop {
        match stream.read(&mut tmp).await? {
            0 => break, // EOF
            n => {
                buf.extend_from_slice(&tmp[..n]);

                // Check if we have complete HTTP headers
                if let Some(header_end) = find_http_header_end(&buf) {
                    let content_length = parse_content_length(&buf[..header_end]);
                    if let Some(body_len) = content_length {
                        // Use checked_add to prevent overflow bypassing the size limit
                        let total_needed = (header_end + 4).checked_add(body_len);
                        if total_needed.is_none_or(|n| n > 1_048_576) {
                            let response = JsonRpcResponse::error(
                                None,
                                INVALID_REQUEST,
                                "Request too large".to_string(),
                            );
                            let json = serde_json::to_string(&response).unwrap_or_default();
                            let http_response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                json.len(),
                                json
                            );
                            stream.write_all(http_response.as_bytes()).await?;
                            return Ok(());
                        }
                        // Read remaining body bytes if needed
                        // unwrap is safe: we verified total_needed is Some above
                        let total_needed = total_needed.unwrap();
                        while buf.len() < total_needed {
                            match stream.read(&mut tmp).await? {
                                0 => break,
                                n => buf.extend_from_slice(&tmp[..n]),
                            }
                        }
                        // Truncate to exactly total_needed so any overshoot
                        // bytes from the final read() don't corrupt parsing.
                        buf.truncate(total_needed);
                        break;
                    }
                }

                // Fallback: try to parse as raw JSON
                if serde_json::from_slice::<serde_json::Value>(&buf).is_ok() {
                    break;
                }
                if buf.len() > 1_048_576 {
                    let response = JsonRpcResponse::error(
                        None,
                        INVALID_REQUEST,
                        "Request too large".to_string(),
                    );
                    let json = serde_json::to_string(&response).unwrap_or_default();
                    let http_response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        json.len(),
                        json
                    );
                    stream.write_all(http_response.as_bytes()).await?;
                    return Ok(());
                }
            }
        }
    }

    if buf.is_empty() {
        return Ok(());
    }

    // Skip HTTP headers if present (for curl compatibility)
    let body = extract_body(&buf);

    // Parse JSON-RPC request
    let request: JsonRpcRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => {
            let response = JsonRpcResponse::error(None, PARSE_ERROR, format!("Parse error: {e}"));
            let json = serde_json::to_string(&response).unwrap_or_default();
            let http_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                json.len(),
                json
            );
            stream.write_all(http_response.as_bytes()).await?;
            return Ok(());
        }
    };

    // Validate jsonrpc version
    if request.jsonrpc != "2.0" {
        let response = JsonRpcResponse::error(
            request.id,
            INVALID_REQUEST,
            "Invalid JSON-RPC version".to_string(),
        );
        let json = serde_json::to_string(&response).unwrap_or_default();
        let http_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            json.len(),
            json
        );
        stream.write_all(http_response.as_bytes()).await?;
        return Ok(());
    }

    // JSON-RPC 2.0 notifications (absent id) must not yield a response.
    if request.id.is_none() {
        return Ok(());
    }

    // Dispatch based on method
    let response = dispatch_request(request, &tx).await;
    let json = serde_json::to_string(&response).unwrap_or_default();

    // Send HTTP response (for curl compatibility)
    let http_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        json.len(),
        json
    );
    stream.write_all(http_response.as_bytes()).await?;

    Ok(())
}

/// Build an arf-only session response, falling back to INTERNAL_ERROR if
/// serialization fails (should never happen, but avoids panics in recovery paths).
fn session_fallback_response(id: Option<serde_json::Value>, reason: &str) -> JsonRpcResponse {
    match serde_json::to_value(super::collect_session_result(false, reason)) {
        Ok(val) => JsonRpcResponse::success(id, val),
        Err(e) => JsonRpcResponse::error(id, INTERNAL_ERROR, format!("Session info error: {e}")),
    }
}

async fn dispatch_request(
    request: JsonRpcRequest,
    tx: &mpsc::Sender<IpcRequest>,
) -> JsonRpcResponse {
    let id = request.id.clone();
    let is_session = request.method == "session";
    let is_history = request.method == "history";

    // Reject immediately if in alternate mode (shell, history/help browser).
    // These modes block the main thread, so requests would hang in the mpsc
    // queue until the request timeout expires.
    //
    // Exceptions: `session` and `history` are handled entirely on the server
    // thread (no main-thread dispatch needed), so they work in alternate mode.
    if super::is_in_alternate_mode() {
        if is_session {
            return session_fallback_response(
                id,
                "R is in alternate mode (shell, history browser, or help browser)",
            );
        }
        if !is_history {
            return JsonRpcResponse::error(
                id,
                super::protocol::R_NOT_AT_PROMPT,
                "R is not at the command prompt".to_string(),
            );
        }
    }

    let method = match request.method.as_str() {
        "evaluate" => {
            let params: EvaluateParams = match serde_json::from_value(request.params) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        id,
                        INVALID_PARAMS,
                        format!("Invalid params: {e}"),
                    );
                }
            };
            IpcMethod::Evaluate {
                code: params.code,
                visible: params.visible,
                timeout_ms: params.timeout_ms,
            }
        }
        "shutdown" => {
            // Shutdown is handled directly on the server thread — no need
            // to send to the main thread. Only available in headless mode.
            if super::trigger_headless_shutdown() {
                return JsonRpcResponse::success(
                    id,
                    serde_json::to_value(ShutdownResult { accepted: true }).unwrap(),
                );
            } else {
                return JsonRpcResponse::error(
                    id,
                    METHOD_NOT_FOUND,
                    "shutdown is only available in headless mode".to_string(),
                );
            }
        }
        "user_input" => {
            let params: UserInputParams = match serde_json::from_value(request.params) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        id,
                        INVALID_PARAMS,
                        format!("Invalid params: {e}"),
                    );
                }
            };
            IpcMethod::UserInput { code: params.code }
        }
        "session" => IpcMethod::Session,
        "history" => {
            // History is handled directly on the server thread — it only
            // reads the SQLite database and does not touch R state.
            // Treat missing/null params as empty object so callers can
            // rely on defaults (all fields have #[serde(default)]).
            let raw_params = if request.params.is_null() {
                serde_json::Value::Object(Default::default())
            } else {
                request.params
            };
            let params: HistoryParams = match serde_json::from_value(raw_params) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        id,
                        INVALID_PARAMS,
                        format!("Invalid params: {e}"),
                    );
                }
            };
            match super::query_history(&params) {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(value) => return JsonRpcResponse::success(id, value),
                    Err(e) => {
                        return JsonRpcResponse::error(
                            id,
                            INTERNAL_ERROR,
                            format!("Failed to serialize history result: {e}"),
                        );
                    }
                },
                Err(super::HistoryQueryError::InvalidParams(message)) => {
                    return JsonRpcResponse::error(id, INVALID_PARAMS, message);
                }
                Err(super::HistoryQueryError::Internal(message)) => {
                    return JsonRpcResponse::error(id, INTERNAL_ERROR, message);
                }
            }
        }
        _ => {
            return JsonRpcResponse::error(
                id,
                METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            );
        }
    };

    // Extract timeout from method (evaluate supports custom timeout).
    // Clamp to a reasonable maximum to avoid overflowing Tokio's internal
    // deadline computations or tying up the server task indefinitely.
    const MAX_TIMEOUT_MS: u64 = 86_400_000; // 24 hours
    // Session info collection is lightweight; use a short timeout.
    const SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    let timeout = match &method {
        IpcMethod::Evaluate { timeout_ms, .. } => match timeout_ms {
            Some(ms) if *ms > MAX_TIMEOUT_MS => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    format!("timeout_ms too large (max {MAX_TIMEOUT_MS} ms, got {ms})"),
                );
            }
            Some(ms) => std::time::Duration::from_millis(*ms),
            None => super::DEFAULT_EVAL_TIMEOUT,
        },
        IpcMethod::Session => SESSION_TIMEOUT,
        _ => super::DEFAULT_EVAL_TIMEOUT,
    };

    // Send to main thread and await response
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let ipc_request = IpcRequest {
        method,
        reply: reply_tx,
    };

    if tx.send(ipc_request).is_err() {
        if is_session {
            // Return arf-only info if main thread is unavailable
            return session_fallback_response(id, "Main thread is unavailable");
        }
        return JsonRpcResponse::error(id, INTERNAL_ERROR, "Main thread unavailable".to_string());
    }

    // Wait for response from main thread (with timeout)
    match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(response)) => match response {
            IpcResponse::Evaluate(result) => {
                JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
            }
            IpcResponse::UserInput(result) => {
                JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
            }
            IpcResponse::Session(result) => {
                JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
            }
            IpcResponse::Error {
                code,
                message,
                data,
            } => {
                let mut resp = JsonRpcResponse::error(id, code, message);
                if let Some(ref mut err) = resp.error {
                    err.data = data;
                }
                resp
            }
        },
        Ok(Err(_)) => {
            if is_session {
                return session_fallback_response(id, "Request handler dropped");
            }
            JsonRpcResponse::error(id, INTERNAL_ERROR, "Request handler dropped".to_string())
        }
        Err(_) => {
            if is_session {
                return session_fallback_response(id, "Timed out collecting R session information");
            }
            JsonRpcResponse::error(id, INTERNAL_ERROR, "Request timed out".to_string())
        }
    }
}

/// Find the position of the end of HTTP headers (`\r\n\r\n`).
/// Returns the byte offset of the first `\r` in the blank line, or None.
fn find_http_header_end(data: &[u8]) -> Option<usize> {
    if data.starts_with(b"POST ") || data.starts_with(b"GET ") || data.starts_with(b"PUT ") {
        data.windows(4).position(|w| w == b"\r\n\r\n")
    } else {
        None
    }
}

/// Parse the Content-Length header value from HTTP headers (case-insensitive).
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let header_str = std::str::from_utf8(headers).ok()?;
    let prefix = "content-length:";
    for line in header_str.split("\r\n") {
        if line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return line[prefix.len()..].trim().parse().ok();
        }
    }
    None
}

/// Extract the body from an HTTP request (skip headers).
/// If the input doesn't look like HTTP, return it as-is.
fn extract_body(data: &[u8]) -> &[u8] {
    // Look for the blank line that separates HTTP headers from body
    if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
        // Only treat as HTTP if it starts with a method keyword
        if data.starts_with(b"POST ") || data.starts_with(b"GET ") || data.starts_with(b"PUT ") {
            return &data[pos + 4..];
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_body_http() {
        let http =
            b"POST / HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"jsonrpc\":\"2.0\"}";
        assert_eq!(extract_body(http), b"{\"jsonrpc\":\"2.0\"}");
    }

    #[test]
    fn test_extract_body_raw_json() {
        let raw = b"{\"jsonrpc\":\"2.0\"}";
        assert_eq!(extract_body(raw), b"{\"jsonrpc\":\"2.0\"}");
    }

    /// Tests that dispatch_request rejects both evaluate and user_input
    /// in alternate mode.
    ///
    /// Serialized with `#[serial]` because all tests that touch the global
    /// `IN_ALTERNATE_MODE` / `R_IS_AT_PROMPT` atomics must not run concurrently.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_dispatch_rejects_in_alternate_mode() {
        use super::super::protocol::R_NOT_AT_PROMPT;

        /// Drop guard that resets global IPC state on scope exit (including panics).
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                super::super::set_in_alternate_mode(false);
            }
        }

        super::super::set_in_alternate_mode(true);
        let _guard = Guard;

        // evaluate should be rejected
        let (tx, _rx) = mpsc::channel();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "evaluate".to_string(),
            params: serde_json::json!({"code": "1+1"}),
            id: Some(serde_json::json!(1)),
        };
        let response = dispatch_request(request, &tx).await;
        assert_eq!(response.error.unwrap().code, R_NOT_AT_PROMPT);

        // user_input should also be rejected
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "user_input".to_string(),
            params: serde_json::json!({"code": "print('hello')"}),
            id: Some(serde_json::json!(2)),
        };
        let response = dispatch_request(request, &tx).await;
        assert_eq!(response.error.unwrap().code, R_NOT_AT_PROMPT);

        // Cleanup handled by Guard drop
    }

    /// Tests that `session` returns arf-only success (not an error) in alternate mode,
    /// with a context-appropriate `r_unavailable_reason`.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_session_returns_arf_only_in_alternate_mode() {
        use super::super::protocol::SessionResult;

        /// Drop guard that resets global IPC state on scope exit (including panics).
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                super::super::set_in_alternate_mode(false);
            }
        }

        super::super::set_in_alternate_mode(true);
        let _guard = Guard;

        let (tx, _rx) = mpsc::channel();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "session".to_string(),
            params: serde_json::json!({}),
            id: Some(serde_json::json!(1)),
        };
        let response = dispatch_request(request, &tx).await;

        // Should be a success, not an error
        assert!(
            response.error.is_none(),
            "session should not return an error"
        );
        let result_value = response.result.expect("session should return a result");
        let result: SessionResult =
            serde_json::from_value(result_value).expect("should parse as SessionResult");

        // Should have arf info
        assert!(!result.arf_version.is_empty());
        assert!(result.pid > 0);

        // R info should be absent with an explanation
        assert!(
            result.r.is_none(),
            "R info should be null in alternate mode"
        );
        let reason = result
            .r_unavailable_reason
            .expect("should have r_unavailable_reason");
        assert!(
            reason.contains("alternate mode"),
            "reason should mention alternate mode, got: {reason}"
        );
        assert!(result.hint.is_some(), "should have a hint");
    }

    /// Tests that `session` returns arf-only info when the main thread channel
    /// is broken (tx.send fails).
    #[tokio::test]
    #[serial_test::serial]
    async fn test_session_fallback_on_channel_failure() {
        use super::super::protocol::SessionResult;

        /// Drop guard that resets global IPC state on scope exit (including panics).
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                super::super::set_in_alternate_mode(false);
            }
        }

        super::super::set_in_alternate_mode(false);
        let _guard = Guard;

        // Create a channel and immediately drop the receiver so send() fails
        let (tx, _rx) = mpsc::channel();
        drop(_rx);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "session".to_string(),
            params: serde_json::json!({}),
            id: Some(serde_json::json!(1)),
        };
        let response = dispatch_request(request, &tx).await;

        // Should be a success with arf-only info
        assert!(
            response.error.is_none(),
            "session should not return an error"
        );
        let result_value = response.result.expect("session should return a result");
        let result: SessionResult =
            serde_json::from_value(result_value).expect("should parse as SessionResult");

        assert!(result.r.is_none(), "R info should be null");
        assert!(
            result.r_unavailable_reason.is_some(),
            "should have r_unavailable_reason"
        );
    }

    /// Tests that `log_file` in `SessionResult` reflects what was passed to `set_session_meta`.
    #[test]
    #[serial_test::serial]
    fn test_session_result_includes_log_file() {
        // With log_file set
        super::super::set_session_meta(
            "/tmp/test.sock".to_string(),
            "2026-01-01T00:00:00+00:00".to_string(),
            Some("/tmp/arf.log".to_string()),
            None,
        );
        let result = super::super::collect_session_result(false, "test");
        assert_eq!(result.log_file.as_deref(), Some("/tmp/arf.log"));

        // Without log_file
        super::super::set_session_meta(
            "/tmp/test.sock".to_string(),
            "2026-01-01T00:00:00+00:00".to_string(),
            None,
            None,
        );
        let result = super::super::collect_session_result(false, "test");
        assert_eq!(result.log_file, None);

        // Verify JSON serialization always includes the field
        let json = serde_json::to_value(&result).unwrap();
        assert!(
            json.get("log_file").is_some(),
            "log_file field should always be present in JSON"
        );
        assert!(
            json["log_file"].is_null(),
            "log_file should be null when not configured"
        );
    }

    /// Tests that `history_session_id` in `SessionResult` reflects what was passed to
    /// `set_session_meta`.
    #[test]
    #[serial_test::serial]
    fn test_session_result_includes_history_session_id() {
        // With history_session_id set
        let session_id: i64 = 1_700_000_000_000_000_000;
        super::super::set_session_meta(
            "/tmp/test.sock".to_string(),
            "2026-01-01T00:00:00+00:00".to_string(),
            None,
            Some(session_id),
        );
        let result = super::super::collect_session_result(false, "test");
        assert_eq!(result.history_session_id, Some(session_id));

        // Verify JSON serialization includes the value
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["history_session_id"], session_id);

        // Without history_session_id (headless mode)
        super::super::set_session_meta(
            "/tmp/test.sock".to_string(),
            "2026-01-01T00:00:00+00:00".to_string(),
            None,
            None,
        );
        let result = super::super::collect_session_result(false, "test");
        assert_eq!(result.history_session_id, None);

        // Verify JSON serialization shows null
        let json = serde_json::to_value(&result).unwrap();
        assert!(
            json["history_session_id"].is_null(),
            "history_session_id should be null when not set"
        );
    }

    #[cfg(unix)]
    mod socket_dir_tests {
        use super::super::{is_dir_safe, select_socket_dir};
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn nonexistent_dir_is_safe() {
            let tmp = tempfile::tempdir().unwrap();
            let candidate = tmp.path().join("does-not-exist");
            assert!(is_dir_safe(&candidate));
        }

        #[test]
        fn dir_with_0700_is_safe() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("good");
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(is_dir_safe(&dir));
        }

        #[test]
        fn dir_with_group_read_is_unsafe() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("leaky");
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
            assert!(!is_dir_safe(&dir));
        }

        #[test]
        fn dir_with_other_write_is_unsafe() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("world");
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o702)).unwrap();
            assert!(!is_dir_safe(&dir));
        }

        #[test]
        fn symlink_is_unsafe() {
            let tmp = tempfile::tempdir().unwrap();
            let target = tmp.path().join("real");
            std::fs::create_dir(&target).unwrap();
            let link = tmp.path().join("link");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(!is_dir_safe(&link));
        }

        #[test]
        fn regular_file_is_unsafe() {
            let tmp = tempfile::tempdir().unwrap();
            let file = tmp.path().join("not-a-dir");
            std::fs::write(&file, "").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(!is_dir_safe(&file));
        }

        #[test]
        fn select_uses_first_safe_candidate() {
            let tmp = tempfile::tempdir().unwrap();
            let good = tmp.path().join("good");
            let also_good = tmp.path().join("also-good");
            // Neither exists yet — both are safe, first should win.
            let result = select_socket_dir(12345, &[good.clone(), also_good.clone()]);
            assert!(result.is_some());
            assert!(result.unwrap().contains("good"));
            assert!(good.exists(), "first candidate should have been created");
            assert!(
                !also_good.exists(),
                "second candidate should not have been created"
            );
        }

        #[test]
        fn select_skips_unsafe_candidate() {
            let tmp = tempfile::tempdir().unwrap();
            let bad = tmp.path().join("bad");
            std::fs::create_dir(&bad).unwrap();
            std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o777)).unwrap();
            let good = tmp.path().join("fallback");
            let result = select_socket_dir(12345, &[bad, good.clone()]);
            assert!(result.is_some());
            assert!(result.unwrap().contains("fallback"));
        }

        #[test]
        fn select_returns_none_when_all_unsafe() {
            let tmp = tempfile::tempdir().unwrap();
            let bad1 = tmp.path().join("bad1");
            std::fs::create_dir(&bad1).unwrap();
            std::fs::set_permissions(&bad1, std::fs::Permissions::from_mode(0o777)).unwrap();
            let bad2 = tmp.path().join("bad2");
            std::fs::write(&bad2, "").unwrap(); // regular file, not a dir
            std::fs::set_permissions(&bad2, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result = select_socket_dir(12345, &[bad1, bad2]);
            assert!(result.is_none());
        }
    }
}
