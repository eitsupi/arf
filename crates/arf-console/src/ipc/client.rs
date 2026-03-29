//! IPC client for the `arf ipc` subcommand.
//!
//! Uses synchronous std I/O — no tokio runtime needed on the client side.
//!
//! All commands output JSON to stdout (pretty-printed when stdout is a
//! terminal, compact when piped). Errors are written to stderr as JSON
//! with `{"error": {"code": N, "message": "...", "hint": "..."}}`.

use crate::ipc::protocol::JsonRpcResponse;
use crate::ipc::session::{find_session, list_sessions};
use anyhow::{Context, Result};

/// Default transport timeout for client-side socket reads (5 minutes).
const DEFAULT_TRANSPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ── Exit codes ───────────────────────────────────────────────────────
//
// 0 = success (implicit via Ok(()))
// 1 = reserved for R evaluation errors (future use)
// 2 = IPC transport error
// 3 = session resolution error
// 4 = JSON-RPC protocol error

/// IPC transport error (socket connection failed, timeout, etc.).
const EXIT_TRANSPORT: i32 = 2;
/// Session resolution error (no session found, ambiguous PID, etc.).
const EXIT_SESSION: i32 = 3;
/// JSON-RPC protocol error (R_BUSY, INPUT_ALREADY_PENDING, etc.).
const EXIT_PROTOCOL: i32 = 4;

// ── Output helpers ───────────────────────────────────────────────────

/// Print a JSON value to stdout. Pretty-prints when stdout is a
/// terminal, compact when piped.
fn print_json(value: &serde_json::Value) {
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let output = if is_tty {
        serde_json::to_string_pretty(value).expect("JSON serialization failed")
    } else {
        serde_json::to_string(value).expect("JSON serialization failed")
    };
    println!("{output}");
}

/// Print a structured error to stderr as JSON and exit with the given code.
///
/// The error format is: `{"error": {"code": N, "message": "...", "hint": "..."}}`
fn exit_error(exit_code: i32, code: i32, message: &str, hint: Option<&str>) -> ! {
    let mut error = serde_json::json!({
        "error": {
            "code": code,
            "message": message,
        }
    });
    if let Some(hint) = hint {
        error["error"]["hint"] = serde_json::Value::String(hint.to_string());
    }
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let output = if is_tty {
        serde_json::to_string_pretty(&error).expect("JSON serialization failed")
    } else {
        serde_json::to_string(&error).expect("JSON serialization failed")
    };
    eprintln!("{output}");
    std::process::exit(exit_code);
}

/// Map a JSON-RPC error code to a hint string.
fn hint_for_rpc_error(code: i32) -> Option<&'static str> {
    use crate::ipc::protocol::*;
    match code {
        R_BUSY => Some(
            "R is executing code. Wait for it to finish, or use \
             'arf ipc session' to check status.",
        ),
        R_NOT_AT_PROMPT => Some(
            "R is not at the prompt (e.g. in browser/menu mode). \
             Complete the current interaction first.",
        ),
        INPUT_ALREADY_PENDING => Some(
            "Another IPC input is already queued. Wait for it to \
             be processed before sending more.",
        ),
        USER_IS_TYPING => Some(
            "The user is typing in the REPL. Wait for them to \
             finish or clear their input.",
        ),
        _ => None,
    }
}

/// Handle a JSON-RPC error response: print structured error and exit.
fn handle_rpc_error(error: &crate::ipc::protocol::JsonRpcError) -> ! {
    let hint = hint_for_rpc_error(error.code);
    exit_error(EXIT_PROTOCOL, error.code, &error.message, hint);
}

/// List all active arf sessions as JSON.
pub fn cmd_list() -> Result<()> {
    let sessions = list_sessions();

    let sessions_json: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            serde_json::json!({
                "pid": s.pid,
                "r_version": s.r_version,
                "socket_path": s.socket_path,
                "cwd": s.cwd,
                "started_at": s.started_at,
                "log_file": s.log_file,
            })
        })
        .collect();

    print_json(&serde_json::json!({ "sessions": sessions_json }));
    Ok(())
}

/// Resolve a session or exit with a structured JSON error.
fn resolve_session(pid: Option<u32>) -> crate::ipc::session::SessionInfo {
    match find_session(pid) {
        Some(session) => session,
        None => {
            if let Some(p) = pid {
                exit_error(
                    EXIT_SESSION,
                    EXIT_SESSION,
                    &format!("No active arf session with PID {p}"),
                    Some("Use 'arf ipc list' to see active sessions."),
                );
            } else {
                let sessions = list_sessions();
                if sessions.is_empty() {
                    exit_error(
                        EXIT_SESSION,
                        EXIT_SESSION,
                        "No active arf sessions found",
                        Some("Start arf with --with-ipc to enable IPC."),
                    );
                } else {
                    exit_error(
                        EXIT_SESSION,
                        EXIT_SESSION,
                        "Multiple arf sessions running",
                        Some(
                            "Specify --pid to select one. Use 'arf ipc list' \
                             to see active sessions.",
                        ),
                    );
                }
            }
        }
    }
}

/// Evaluate R code in a running arf session.
///
/// On success, prints the structured result as JSON to stdout.
/// R evaluation errors are returned as part of the JSON result (exit 0)
/// — they are a normal response, not an IPC failure.
pub fn cmd_eval(
    code: &str,
    pid: Option<u32>,
    visible: bool,
    timeout_ms: Option<u64>,
) -> Result<()> {
    let session = resolve_session(pid);

    let mut params = serde_json::json!({ "code": code, "visible": visible });
    if let Some(ms) = timeout_ms {
        params["timeout_ms"] = serde_json::json!(ms);
    }

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "evaluate",
        "params": params
    });

    // Client transport timeout: match the server-side timeout with a small buffer
    // so the server can respond with a proper timeout error before the client gives up.
    let transport_timeout = match timeout_ms {
        Some(ms) => std::time::Duration::from_millis(ms.saturating_add(5000)),
        None => DEFAULT_TRANSPORT_TIMEOUT + std::time::Duration::from_secs(5),
    };

    let response = send_request(&session.socket_path, &request, transport_timeout);

    if let Some(ref error) = response.error {
        handle_rpc_error(error);
    }

    match response.result {
        Some(result) => {
            print_json(&result);
            Ok(())
        }
        None => {
            exit_error(
                EXIT_PROTOCOL,
                EXIT_PROTOCOL,
                "Server returned empty response",
                None,
            );
        }
    }
}

/// Send code as user input to a running arf session.
pub fn cmd_send(code: &str, pid: Option<u32>) -> Result<()> {
    let session = resolve_session(pid);

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "user_input",
        "params": { "code": code }
    });

    let response = send_request(&session.socket_path, &request, DEFAULT_TRANSPORT_TIMEOUT);

    if let Some(ref error) = response.error {
        handle_rpc_error(error);
    }

    match response.result {
        Some(result) => {
            print_json(&result);
            Ok(())
        }
        None => {
            exit_error(
                EXIT_PROTOCOL,
                EXIT_PROTOCOL,
                "Server returned empty response",
                None,
            );
        }
    }
}

/// Shut down a running arf headless session.
pub fn cmd_shutdown(pid: Option<u32>) -> Result<()> {
    let session = resolve_session(pid);

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "shutdown",
        "params": {}
    });

    let response = send_request(&session.socket_path, &request, DEFAULT_TRANSPORT_TIMEOUT);

    if let Some(ref error) = response.error {
        handle_rpc_error(error);
    }

    match response.result {
        Some(result) => {
            print_json(&result);
            Ok(())
        }
        None => {
            exit_error(
                EXIT_PROTOCOL,
                EXIT_PROTOCOL,
                "Server returned empty response",
                None,
            );
        }
    }
}

/// Get session information as JSON via the `session` IPC method.
///
/// Output is pretty-printed when stdout is a terminal, compact when piped.
pub fn cmd_session(pid: Option<u32>) -> Result<()> {
    let session = resolve_session(pid);

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session",
        "params": {}
    });

    // Session info collection is lightweight; use a short transport timeout.
    let transport_timeout = std::time::Duration::from_secs(15);

    let response = send_request(&session.socket_path, &request, transport_timeout);

    if let Some(ref error) = response.error {
        handle_rpc_error(error);
    }

    match response.result {
        Some(result) => {
            print_json(&result);
            Ok(())
        }
        None => {
            exit_error(
                EXIT_PROTOCOL,
                EXIT_PROTOCOL,
                "Server returned empty response",
                None,
            );
        }
    }
}

/// Query command history via the `history` IPC method.
///
/// Output is pretty-printed when stdout is a terminal, compact when piped.
pub fn cmd_history(
    pid: Option<u32>,
    limit: i64,
    all_sessions: bool,
    cwd: Option<&str>,
    grep: Option<&str>,
    since: Option<&str>,
) -> Result<()> {
    let session = resolve_session(pid);

    let mut params = serde_json::json!({
        "limit": limit,
        "all_sessions": all_sessions,
    });
    if let Some(cwd) = cwd {
        params["cwd"] = serde_json::Value::String(cwd.to_string());
    }
    if let Some(grep) = grep {
        params["grep"] = serde_json::Value::String(grep.to_string());
    }
    if let Some(since) = since {
        params["since"] = serde_json::Value::String(since.to_string());
    }

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "history",
        "params": params
    });

    let transport_timeout = std::time::Duration::from_secs(15);
    let response = send_request(&session.socket_path, &request, transport_timeout);

    if let Some(ref error) = response.error {
        handle_rpc_error(error);
    }

    match response.result {
        Some(result) => {
            print_json(&result);
            Ok(())
        }
        None => {
            exit_error(
                EXIT_PROTOCOL,
                EXIT_PROTOCOL,
                "Server returned empty response",
                None,
            );
        }
    }
}

/// Send a JSON-RPC request to the socket and return the response.
///
/// On transport errors (connection refused, timeout, etc.), exits with
/// a structured JSON error on stderr.
fn send_request(
    socket_path: &str,
    request: &serde_json::Value,
    timeout: std::time::Duration,
) -> JsonRpcResponse {
    match send_request_inner(socket_path, request, timeout) {
        Ok(response) => response,
        Err(e) => {
            exit_error(
                EXIT_TRANSPORT,
                EXIT_TRANSPORT,
                &format!("{e:#}"),
                Some("Check that the arf session is running and IPC is enabled."),
            );
        }
    }
}

/// Inner transport implementation that returns Result for ergonomic error handling.
fn send_request_inner(
    socket_path: &str,
    request: &serde_json::Value,
    timeout: std::time::Duration,
) -> Result<JsonRpcResponse> {
    let body = serde_json::to_string(request)?;

    #[cfg(unix)]
    {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;

        let http_request = format!(
            "POST / HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n{}",
            body.len(),
            body
        );

        let mut stream = UnixStream::connect(socket_path)
            .with_context(|| format!("Failed to connect to {socket_path}"))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.write_all(http_request.as_bytes())?;
        stream.shutdown(std::net::Shutdown::Write)?;

        let mut response_buf = Vec::new();
        stream.read_to_end(&mut response_buf)?;

        parse_http_response(&response_buf)
    }

    #[cfg(windows)]
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::windows::named_pipe::ClientOptions;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create tokio runtime")?;

        rt.block_on(async {
            let mut pipe = ClientOptions::new()
                .open(socket_path)
                .with_context(|| format!("Failed to connect to {socket_path}"))?;

            // Send raw JSON (no HTTP wrapping) — the server detects complete
            // JSON and stops reading, so no write shutdown is needed.
            pipe.write_all(body.as_bytes()).await?;
            pipe.flush().await?;

            // Read response with timeout
            let mut response_buf = Vec::new();
            match tokio::time::timeout(timeout, pipe.read_to_end(&mut response_buf)).await {
                Ok(result) => result?,
                Err(_) => anyhow::bail!("Request timed out after {}s", timeout.as_secs()),
            };

            parse_http_response(&response_buf)
        })
    }
}

/// Parse an HTTP response and extract the JSON body.
fn parse_http_response(data: &[u8]) -> Result<JsonRpcResponse> {
    let text = String::from_utf8_lossy(data);

    let body = if let Some(pos) = text.find("\r\n\r\n") {
        &text[pos + 4..]
    } else {
        &text
    };

    serde_json::from_str(body).context("Failed to parse JSON-RPC response")
}
