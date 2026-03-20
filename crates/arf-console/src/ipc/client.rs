//! IPC client for the `arf ipc` subcommand.
//!
//! Uses synchronous std I/O — no tokio runtime needed on the client side.

use crate::ipc::protocol::JsonRpcResponse;
use crate::ipc::session::{find_session, list_sessions};
use anyhow::{Context, Result};

/// Default transport timeout for client-side socket reads (5 minutes).
const DEFAULT_TRANSPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// List all active arf sessions.
pub fn cmd_list() -> Result<()> {
    let sessions = list_sessions();

    if sessions.is_empty() {
        println!("No active arf sessions found.");
        println!("Start arf with --with-ipc to enable IPC.");
        return Ok(());
    }

    println!("{:<8} {:<12} CWD", "PID", "R VERSION");
    println!("{}", "-".repeat(60));
    for session in &sessions {
        println!(
            "{:<8} {:<12} {}",
            session.pid,
            session.r_version.as_deref().unwrap_or("?"),
            session.cwd
        );
    }

    Ok(())
}

/// Resolve a session or return a descriptive error.
fn resolve_session(pid: Option<u32>) -> Result<crate::ipc::session::SessionInfo> {
    find_session(pid).ok_or_else(|| {
        if let Some(p) = pid {
            anyhow::anyhow!("No active arf session with PID {p}")
        } else {
            let sessions = list_sessions();
            if sessions.is_empty() {
                anyhow::anyhow!("No active arf sessions. Start arf with --with-ipc to enable IPC.")
            } else {
                anyhow::anyhow!(
                    "Multiple arf sessions running. Specify --pid to select one.\n\
                     Use `arf ipc list` to see active sessions."
                )
            }
        }
    })
}

/// Evaluate R code in a running arf session.
pub fn cmd_eval(
    code: &str,
    pid: Option<u32>,
    visible: bool,
    timeout_ms: Option<u64>,
) -> Result<()> {
    let session = resolve_session(pid)?;

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

    let response = send_request(&session.socket_path, &request, transport_timeout)?;

    if let Some(error) = response.error {
        eprintln!("Error: {} (code {})", error.message, error.code);
        std::process::exit(1);
    }

    if let Some(result) = response.result {
        if let Some(stdout) = result
            .get("stdout")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            print!("{stdout}");
            if !stdout.ends_with('\n') {
                println!();
            }
        }

        if let Some(stderr) = result
            .get("stderr")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            eprint!("{stderr}");
            if !stderr.ends_with('\n') {
                eprintln!();
            }
        }

        if let Some(value) = result
            .get("value")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            println!("{value}");
        }

        if let Some(error) = result
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            eprintln!("Error in R: {error}");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Send code as user input to a running arf session.
pub fn cmd_send(code: &str, pid: Option<u32>) -> Result<()> {
    let session = resolve_session(pid)?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "user_input",
        "params": { "code": code }
    });

    let response = send_request(&session.socket_path, &request, DEFAULT_TRANSPORT_TIMEOUT)?;

    if let Some(error) = response.error {
        eprintln!("Error: {} (code {})", error.message, error.code);
        std::process::exit(1);
    }

    if let Some(result) = response.result
        && result.get("accepted").and_then(|v| v.as_bool()) == Some(true)
    {
        println!("Input accepted.");
    }

    Ok(())
}

/// Shut down a running arf headless session.
pub fn cmd_shutdown(pid: Option<u32>) -> Result<()> {
    let session = resolve_session(pid)?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "shutdown",
        "params": {}
    });

    let response = send_request(&session.socket_path, &request, DEFAULT_TRANSPORT_TIMEOUT)?;

    if let Some(error) = response.error {
        eprintln!("Error: {} (code {})", error.message, error.code);
        std::process::exit(1);
    }

    match response.result {
        Some(result) => {
            if result.get("accepted").and_then(|v| v.as_bool()) == Some(true) {
                println!("Shutdown request accepted.");
            } else {
                eprintln!("Shutdown request was not accepted by the server.");
                std::process::exit(1);
            }
        }
        None => {
            eprintln!(
                "Server response did not contain a result; shutdown may not have been processed."
            );
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Show status of a specific session.
pub fn cmd_status(pid: Option<u32>) -> Result<()> {
    let session = find_session(pid).ok_or_else(|| {
        anyhow::anyhow!("No matching arf session found. Use `arf ipc list` to see active sessions.")
    })?;

    println!("PID:        {}", session.pid);
    println!(
        "R version:  {}",
        session.r_version.as_deref().unwrap_or("unknown")
    );
    println!("Socket:     {}", session.socket_path);
    println!("CWD:        {}", session.cwd);
    println!("Started:    {}", session.started_at);

    Ok(())
}

/// Send a JSON-RPC request to the socket and return the response.
fn send_request(
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
                Err(_) => anyhow::bail!("Request timed out after {}ms", timeout.as_millis()),
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
