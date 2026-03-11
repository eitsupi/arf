//! IPC server that listens on a Unix socket (or TCP fallback).
//!
//! Runs in a dedicated thread with a tokio current_thread runtime.
//! Each connection is handled as a simple HTTP-like JSON-RPC endpoint:
//! read one request, dispatch via mpsc channel, await oneshot reply, respond.

use crate::ipc::protocol::{
    EvaluateParams, INVALID_PARAMS, INVALID_REQUEST, IpcMethod, IpcRequest, IpcResponse,
    JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND, PARSE_ERROR, UserInputParams,
};
use crate::ipc::session::{SessionInfo, remove_session, sessions_dir, write_session};
use std::sync::mpsc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    let tx_clone = tx;

    std::thread::Builder::new()
        .name("arf-ipc-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for IPC server");

            rt.block_on(async move {
                if let Err(e) = run_server(&path, tx_clone).await {
                    log::error!("IPC server error: {}", e);
                }
            });
        })?;

    // Write session metadata
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let r_version = {
        let tmpfile = std::env::temp_dir().join(".arf_ipc_rver.txt");
        let tmppath = tmpfile.display().to_string().replace('\\', "/");
        let code =
            format!(r#"writeLines(paste0(R.version$major, ".", R.version$minor), "{tmppath}")"#);
        let _ = arf_harp::eval_string(&code);
        let ver = std::fs::read_to_string(&tmpfile).ok();
        let _ = std::fs::remove_file(&tmpfile);
        ver.map(|s| s.trim().to_string())
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

/// Stop the IPC server (cleanup).
pub fn stop_server() {
    let pid = std::process::id();
    let socket_path = get_socket_path(pid);

    // Remove socket file
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&socket_path);
    }

    // Remove session metadata
    remove_session(pid);
}

/// Get the socket path for a given PID.
fn get_socket_path(pid: u32) -> String {
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

/// Run the actual server loop.
#[cfg(unix)]
async fn run_server(socket_path: &str, tx: mpsc::Sender<IpcRequest>) -> std::io::Result<()> {
    let listener = tokio::net::UnixListener::bind(socket_path)?;
    log::info!("IPC server listening on {}", socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, tx).await {
                        log::debug!("IPC connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                log::warn!("IPC accept error: {}", e);
            }
        }
    }
}

#[cfg(windows)]
async fn run_server(socket_path: &str, tx: mpsc::Sender<IpcRequest>) -> std::io::Result<()> {
    // On Windows, fall back to TCP on localhost
    let addr = format!("127.0.0.1:{}", get_tcp_port(std::process::id()));
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    log::info!("IPC server listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, tx).await {
                        log::debug!("IPC connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                log::warn!("IPC accept error: {}", e);
            }
        }
    }
}

#[cfg(windows)]
fn get_tcp_port(pid: u32) -> u16 {
    // Use a deterministic port based on PID in the dynamic range
    (49152 + (pid % 16383)) as u16
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
                    stream.write_all(json.as_bytes()).await?;
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
            IpcMethod::Evaluate { code: params.code }
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
        return JsonRpcResponse::error(id, -32003, "Main thread unavailable".to_string());
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
        Ok(Err(_)) => JsonRpcResponse::error(id, -32003, "Request handler dropped".to_string()),
        Err(_) => JsonRpcResponse::error(id, -32003, "Request timed out".to_string()),
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
}
