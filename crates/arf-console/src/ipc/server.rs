//! IPC server that listens on a Unix socket (or named pipe on Windows).
//!
//! Runs in a dedicated thread with a tokio current_thread runtime.
//! Each connection is handled as a simple HTTP-like JSON-RPC endpoint:
//! read one request, dispatch via mpsc channel, await oneshot reply, respond.

use crate::editor::validator::RValidator;
use crate::ipc::protocol::{
    EvaluateParams, HistoryParams, INCOMPLETE_INPUT, INTERNAL_ERROR, INVALID_PARAMS,
    INVALID_REQUEST, IpcMethod, IpcRequest, IpcResponse, JsonRpcRequest, JsonRpcResponse,
    METHOD_NOT_FOUND, PARSE_ERROR, ShutdownResult, UserInputParams,
};
use crate::ipc::session::{SessionInfo, SessionType, remove_session, write_session};
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
    /// Whether the socket directory was auto-created (not a custom `--ipc-bind`
    /// path).  Only auto-created directories are cleaned up on shutdown.
    #[cfg_attr(windows, allow(dead_code))]
    auto_created_dir: bool,
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
    session_type: SessionType,
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
    let (socket_path, dir_created) = match bind {
        Some(path) => (path.to_string(), false),
        None => get_socket_path(pid).ok_or_else(|| {
            std::io::Error::other(format!(
                "Failed to determine a safe IPC socket path for pid {pid}. \
                 All candidate directories were unsafe or could not be created. \
                 Check the log for details."
            ))
        })?,
    };

    // Remove stale socket file if it exists. When a custom --ipc-bind path is
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
        auto_created_dir: dir_created,
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
        session_type: Some(session_type),
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

/// Get the socket/pipe path for a given PID.
///
/// On Unix, uses `$XDG_RUNTIME_DIR/arf/<pid>.sock` (the XDG-correct location
/// for runtime sockets).  Falls back to `<temp_dir>/arf-<random>/<pid>.sock`
/// when `XDG_RUNTIME_DIR` is not set or its directory fails safety validation.
///
/// The socket directory is validated for safety (not a symlink, owned by
/// the current user, not writable by group/other).
/// Returns `(socket_path, dir_created)` where `dir_created` is `true` when
/// the socket directory was freshly created by this call (and should be
/// cleaned up on shutdown).
fn get_socket_path(pid: u32) -> Option<(String, bool)> {
    #[cfg(unix)]
    {
        let temp_fallback = || {
            let suffix = random_hex_suffix();
            std::env::temp_dir().join(format!("arf-{suffix}"))
        };
        let mut candidates = Vec::with_capacity(2);
        if let Some(runtime_dir) = dirs::runtime_dir() {
            candidates.push(runtime_dir.join("arf"));
        }
        candidates.push(temp_fallback());
        select_socket_dir(pid, &candidates)
    }
    #[cfg(windows)]
    {
        Some((format!(r"\\.\pipe\arf-ipc-{pid}"), false))
    }
}

/// Generate a short random hex string for use in directory names.
///
/// Uses `HashMap`'s `RandomState` (seeded from platform randomness in
/// the standard library) to avoid adding a `rand` dependency.  The
/// result is 16 hex characters, which is sufficient to make directory
/// names unpredictable in practice.
#[cfg(unix)]
fn random_hex_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    let state = std::collections::hash_map::RandomState::new();
    let hash = state.build_hasher().finish();
    format!("{hash:016x}")
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
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o700 {
                log::warn!(
                    "Socket directory {} must have permissions 0700, found {:o}",
                    dir.display(),
                    mode
                );
                return false;
            }
            true
        }
    }
}

/// Try each candidate directory in order, returning the socket path and
/// whether the directory was created by this call.  Creates the chosen
/// directory with mode `0700` if it does not exist.
#[cfg(unix)]
fn select_socket_dir(pid: u32, candidates: &[std::path::PathBuf]) -> Option<(String, bool)> {
    use std::os::unix::fs::DirBuilderExt;

    for dir in candidates {
        if is_dir_safe(dir) {
            // Attempt a non-recursive mkdir to atomically determine
            // whether we created the directory.  Parent directories
            // (e.g. $XDG_RUNTIME_DIR, $TMPDIR) are expected to exist.
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            let created = match builder.create(dir) {
                Ok(()) => true,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(e) => {
                    log::warn!("Failed to create directory {}: {e}", dir.display());
                    continue;
                }
            };
            // Re-validate after creation to close the TOCTOU window: if
            // another process created the directory between our initial
            // check and DirBuilder::create, it may have different
            // ownership or permissions.
            if !is_dir_safe(dir) {
                log::warn!(
                    "Socket directory {} failed safety validation after creation",
                    dir.display()
                );
                continue;
            }
            let path = dir.join(format!("{pid}.sock")).display().to_string();
            return Some((path, created));
        }
    }

    let dirs: Vec<_> = candidates.iter().map(|d| d.display().to_string()).collect();
    log::error!(
        "All socket directory candidates failed (unsafe or could not be created): {}. \
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
            // but custom --ipc-bind paths inherit the parent dir's umask.
            // Use fd-based fchmod to avoid TOCTOU symlink race.
            //
            // NOTE: There is a brief race window between bind() and fchmod()
            // where the socket exists with umask-inherited permissions. For
            // custom --ipc-bind paths in shared directories, operators should
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
            if let Some(response) = incomplete_input_response(id.clone(), &params.code) {
                return response;
            }
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
            if let Some(response) = incomplete_input_response(id.clone(), &params.code) {
                return response;
            }
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

/// Reject code that would make R wait for continuation input.
fn incomplete_input_response(id: Option<serde_json::Value>, code: &str) -> Option<JsonRpcResponse> {
    if RValidator::new().is_complete(code) {
        return None;
    }

    Some(JsonRpcResponse::error(
        id,
        INCOMPLETE_INPUT,
        "R code is syntactically incomplete".to_string(),
    ))
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
mod tests;
