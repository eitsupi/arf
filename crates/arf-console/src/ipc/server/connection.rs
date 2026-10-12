//! Per-connection request handling: HTTP framing, raw JSON-RPC, and response
//! writing.

use super::dispatch::dispatch_request;
use crate::ipc::protocol::{
    INVALID_REQUEST, IpcRequest, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR,
};
use std::sync::mpsc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum accepted request size (1MB).
const MAX_REQUEST_BYTES: usize = 1_048_576;

/// Outcome of reading one request from a stream.
enum RequestRead {
    /// The request bytes, possibly empty if the peer closed without sending.
    Complete(Vec<u8>),
    /// The request exceeded [`MAX_REQUEST_BYTES`].
    TooLarge,
}

/// Handle a single connection: read request, dispatch, respond.
pub(super) async fn handle_connection<S>(
    mut stream: S,
    tx: mpsc::Sender<IpcRequest>,
) -> std::io::Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let buf = match read_request_bytes(&mut stream).await? {
        RequestRead::Complete(buf) => buf,
        RequestRead::TooLarge => {
            let response =
                JsonRpcResponse::error(None, INVALID_REQUEST, "Request too large".to_string());
            return write_response(&mut stream, &response).await;
        }
    };

    if buf.is_empty() {
        return Ok(());
    }

    // Skip HTTP headers if present (for curl compatibility)
    let request = match parse_request(extract_body(&buf)) {
        Ok(request) => request,
        Err(response) => return write_response(&mut stream, &response).await,
    };

    // JSON-RPC 2.0 notifications (absent id) must not yield a response.
    if request.id.is_none() {
        return Ok(());
    }

    // Dispatch based on method
    let response = dispatch_request(request, &tx).await;

    // Send HTTP response (for curl compatibility)
    write_response(&mut stream, &response).await
}

/// Read the full request (up to 1MB).
///
/// Two strategies:
/// 1. HTTP request with Content-Length: read headers, then read exactly body_len bytes.
/// 2. Raw JSON: read until the buffer parses as valid JSON, or EOF.
async fn read_request_bytes<S>(stream: &mut S) -> std::io::Result<RequestRead>
where
    S: AsyncReadExt + Unpin,
{
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
                        if total_needed.is_none_or(|n| n > MAX_REQUEST_BYTES) {
                            return Ok(RequestRead::TooLarge);
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
                if buf.len() > MAX_REQUEST_BYTES {
                    return Ok(RequestRead::TooLarge);
                }
            }
        }
    }

    Ok(RequestRead::Complete(buf))
}

/// Parse and validate a JSON-RPC request body.
///
/// `Err` carries the error response to send back.
fn parse_request(body: &[u8]) -> Result<JsonRpcRequest, Box<JsonRpcResponse>> {
    let request: JsonRpcRequest = serde_json::from_slice(body).map_err(|e| {
        Box::new(JsonRpcResponse::error(
            None,
            PARSE_ERROR,
            format!("Parse error: {e}"),
        ))
    })?;

    // Validate jsonrpc version
    if request.jsonrpc != "2.0" {
        return Err(Box::new(JsonRpcResponse::error(
            request.id,
            INVALID_REQUEST,
            "Invalid JSON-RPC version".to_string(),
        )));
    }

    Ok(request)
}

/// Write a JSON-RPC response as an HTTP response (for curl compatibility).
async fn write_response<S>(stream: &mut S, response: &JsonRpcResponse) -> std::io::Result<()>
where
    S: AsyncWriteExt + Unpin,
{
    let json = serde_json::to_string(response).unwrap_or_default();
    let http_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        json.len(),
        json
    );
    stream.write_all(http_response.as_bytes()).await
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
        assert_eq!(extract_body(http), br#"{"jsonrpc":"2.0"}"#);
    }

    #[test]
    fn test_extract_body_raw_json() {
        let raw = br#"{"jsonrpc":"2.0"}"#;
        assert_eq!(extract_body(raw), br#"{"jsonrpc":"2.0"}"#);
    }
}
