//! IPC integration tests for arf.
//!
//! Tests the IPC `user_input` method via PTY, verifying that external tools
//! can inject input into the R REPL through the break signal mechanism.
//!
//! Unix-only (same as other PTY tests).

mod common;

#[cfg(unix)]
mod ipc_tests {
    use super::common::Terminal;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    /// Find the IPC socket path by scanning the session directory.
    /// Retries until a session file appears or timeout is reached.
    fn find_socket_path(timeout: Duration) -> Option<String> {
        let sessions_dir = dirs::cache_dir()?.join("arf").join("sessions");
        let start = Instant::now();

        while start.elapsed() < timeout {
            if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "json") {
                        if let Ok(contents) = std::fs::read_to_string(&path) {
                            if let Ok(info) = serde_json::from_str::<serde_json::Value>(&contents) {
                                if let Some(socket) =
                                    info.get("socket_path").and_then(|v| v.as_str())
                                {
                                    // Verify the socket is connectable
                                    if UnixStream::connect(socket).is_ok() {
                                        return Some(socket.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        None
    }

    /// Send a JSON-RPC request to the IPC socket and return the response.
    fn send_ipc_request(
        socket_path: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        });

        let body = serde_json::to_string(&request).map_err(|e| e.to_string())?;

        let http_request = format!(
            "POST / HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            body.len(),
            body
        );

        let mut stream =
            UnixStream::connect(socket_path).map_err(|e| format!("Connect failed: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        stream
            .write_all(http_request.as_bytes())
            .map_err(|e| format!("Write failed: {e}"))?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|e| format!("Shutdown failed: {e}"))?;

        let mut response_buf = Vec::new();
        stream
            .read_to_end(&mut response_buf)
            .map_err(|e| format!("Read failed: {e}"))?;

        let text = String::from_utf8_lossy(&response_buf);
        let json_body = if let Some(pos) = text.find("\r\n\r\n") {
            &text[pos + 4..]
        } else {
            &text
        };

        serde_json::from_str(json_body).map_err(|e| format!("Parse failed: {e}: {json_body}"))
    }

    /// Test that IPC `user_input` injects code into the R REPL.
    ///
    /// This verifies the full break signal flow:
    /// 1. arf starts with `--with-ipc`
    /// 2. R is initialized and waiting at the prompt
    /// 3. External IPC client sends `user_input` with R code
    /// 4. reedline's `read_line()` is interrupted via break signal
    /// 5. The code is executed by R and output appears in the terminal
    #[test]
    fn test_ipc_user_input() {
        let mut terminal =
            Terminal::spawn_with_args(&["--with-ipc"]).expect("Failed to spawn arf with IPC");

        // Wait for R to initialize and show the prompt
        terminal
            .wait_for_prompt()
            .expect("Should show prompt after startup");

        // Find the IPC socket (may take a moment for the session file to appear)
        let socket_path = find_socket_path(Duration::from_secs(10))
            .expect("Should find IPC socket path in session directory");

        // Send user_input via IPC — this should trigger the break signal,
        // interrupt read_line(), and feed the code to R
        let response = send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": "cat('ipc_test_output')" }),
        )
        .expect("IPC request should succeed");

        // Verify the IPC response indicates acceptance
        assert!(
            response
                .get("result")
                .and_then(|r| r.get("accepted"))
                .and_then(|a| a.as_bool())
                == Some(true),
            "user_input should be accepted, got: {response:?}"
        );

        // Verify the R output appears in the terminal
        terminal
            .expect("ipc_test_output")
            .expect("R should execute the injected code and show output");

        // Verify we get back to the prompt after execution
        terminal
            .wait_for_prompt()
            .expect("Should return to prompt after IPC input execution");

        terminal.quit().expect("Should quit cleanly");
    }
}
