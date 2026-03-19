//! IPC server that listens on a Unix socket (or named pipe on Windows).
//!
//! Runs in a dedicated thread with a tokio current_thread runtime.
//! Each connection is handled as a simple HTTP-like JSON-RPC endpoint:
//! read one request, dispatch via mpsc channel, await oneshot reply, respond.

use crate::ipc::protocol::{
    EvaluateParams, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, IpcMethod, IpcRequest,
    IpcResponse, JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND, PARSE_ERROR, UserInputParams,
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
/// Returns the socket path on success.
pub fn start_server(tx: mpsc::Sender<IpcRequest>) -> std::io::Result<String> {
    let pid = std::process::id();
    let socket_path = get_socket_path(pid);

    // Remove stale socket file if it exists
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&socket_path);
    }

    let path = socket_path.clone();
    let cancel_token = CancellationToken::new();
    let token_clone = cancel_token.clone();

    let join_handle = std::thread::Builder::new()
        .name("arf-ipc-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for IPC server");

            rt.block_on(async move {
                if let Err(e) = run_server(&path, tx, token_clone).await {
                    log::error!("IPC server error: {}", e);
                }
            });
        })?;

    // Store handle for later shutdown
    let state = ServerState {
        cancel_token,
        join_handle,
        socket_path: socket_path.clone(),
    };
    let handle_store = SERVER_HANDLE.get_or_init(|| Mutex::new(None));
    *handle_store.lock().unwrap() = Some(state);

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
        started_at: chrono::Local::now().to_rfc3339(),
    };

    if let Err(e) = write_session(&session) {
        log::warn!("Failed to write session file: {}", e);
    }

    Ok(socket_path)
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
fn get_socket_path(pid: u32) -> String {
    #[cfg(unix)]
    {
        use crate::ipc::session::sessions_dir;
        if let Some(dir) = sessions_dir() {
            let _ = std::fs::create_dir_all(&dir);
            dir.join(format!("{pid}.sock")).display().to_string()
        } else {
            // Fallback to temp dir
            std::env::temp_dir()
                .join(format!("arf-{pid}.sock"))
                .display()
                .to_string()
        }
    }
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\arf-ipc-{pid}")
    }
}

/// Run the actual server loop.
#[cfg(unix)]
async fn run_server(
    socket_path: &str,
    tx: mpsc::Sender<IpcRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let listener = tokio::net::UnixListener::bind(socket_path)?;
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
    tx: mpsc::Sender<IpcRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    log::info!("IPC server listening on {}", socket_path);

    // Create the first pipe instance
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(socket_path)?;

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
    // Read the full request (up to 1MB)
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];

    // Read until we get a complete JSON object or connection closes.
    // For simplicity, we read until the connection's read side is done
    // or we have enough data. Clients should send the full request
    // and then we respond.
    loop {
        match stream.read(&mut tmp).await? {
            0 => break, // EOF
            n => {
                buf.extend_from_slice(&tmp[..n]);
                // Try to parse as JSON to see if we have a complete request
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

/// Dispatch a JSON-RPC request to the main thread.
async fn dispatch_request(
    request: JsonRpcRequest,
    tx: &mpsc::Sender<IpcRequest>,
) -> JsonRpcResponse {
    let id = request.id.clone();

    // Reject immediately if in alternate mode (shell, history/help browser).
    // These modes block the main thread, so requests would hang in the mpsc
    // queue until the 300s timeout.
    if super::is_in_alternate_mode() {
        return JsonRpcResponse::error(
            id,
            super::protocol::R_NOT_AT_PROMPT,
            "R is not at the command prompt".to_string(),
        );
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
        _ => {
            return JsonRpcResponse::error(
                id,
                METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            );
        }
    };

    // Send to main thread and await response
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let ipc_request = IpcRequest {
        method,
        reply: reply_tx,
    };

    if tx.send(ipc_request).is_err() {
        return JsonRpcResponse::error(id, INTERNAL_ERROR, "Main thread unavailable".to_string());
    }

    // Wait for response from main thread (with timeout)
    match tokio::time::timeout(std::time::Duration::from_secs(300), reply_rx).await {
        Ok(Ok(response)) => match response {
            IpcResponse::Evaluate(result) => {
                JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
            }
            IpcResponse::UserInput(result) => {
                JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
            }
            IpcResponse::Error { code, message } => JsonRpcResponse::error(id, code, message),
        },
        Ok(Err(_)) => {
            JsonRpcResponse::error(id, INTERNAL_ERROR, "Request handler dropped".to_string())
        }
        Err(_) => JsonRpcResponse::error(id, INTERNAL_ERROR, "Request timed out".to_string()),
    }
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
    /// Combined into a single test to avoid flakiness from parallel test
    /// execution, since all tests share the global `IN_ALTERNATE_MODE` atomic.
    #[tokio::test]
    async fn test_dispatch_rejects_in_alternate_mode() {
        use super::super::protocol::R_NOT_AT_PROMPT;

        super::super::set_in_alternate_mode(true);

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

        // Cleanup
        super::super::set_in_alternate_mode(false);
    }
}
