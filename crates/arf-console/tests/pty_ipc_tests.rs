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
    /// Filters by PID to avoid connecting to the wrong session in parallel test runs.
    /// Retries until a matching session file appears or timeout is reached.
    fn find_socket_path(pid: Option<u32>, timeout: Duration) -> Option<String> {
        let sessions_dir = dirs::cache_dir()?.join("arf").join("sessions");
        let start = Instant::now();

        while start.elapsed() < timeout {
            if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "json")
                        && let Ok(contents) = std::fs::read_to_string(&path)
                        && let Ok(info) = serde_json::from_str::<serde_json::Value>(&contents)
                    {
                        // Filter by PID if specified
                        if let Some(target_pid) = pid
                            && info.get("pid").and_then(|v| v.as_u64())
                                != Some(u64::from(target_pid))
                        {
                            continue;
                        }
                        if let Some(socket) = info.get("socket_path").and_then(|v| v.as_str()) {
                            // Verify the socket is connectable
                            if UnixStream::connect(socket).is_ok() {
                                return Some(socket.to_string());
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
             \r\n{}",
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

    /// Helper to spawn arf with IPC and return (terminal, socket_path).
    fn spawn_ipc_session() -> (Terminal, String) {
        let mut terminal =
            Terminal::spawn_with_args(&["--with-ipc"]).expect("Failed to spawn arf with IPC");

        terminal
            .wait_for_prompt()
            .expect("Should show prompt after startup");

        let socket_path = find_socket_path(terminal.process_id(), Duration::from_secs(10))
            .expect("Should find IPC socket path in session directory");

        (terminal, socket_path)
    }

    /// Test that IPC `evaluate` captures stdout, value, and error correctly.
    ///
    /// Verifies the WriteConsoleEx callback capture approach:
    /// - cat() output goes to stdout (via WriteConsoleEx)
    /// - visible value is captured via capture.output(print())
    /// - errors are captured via tryCatch
    /// - ANSI escapes are stripped from captured output
    #[test]
    fn test_ipc_evaluate_capture() {
        let (mut terminal, socket_path) = spawn_ipc_session();

        // Test 1: Simple value capture
        let response = send_ipc_request(
            &socket_path,
            "evaluate",
            serde_json::json!({ "code": "1 + 1" }),
        )
        .expect("evaluate should succeed");

        let result = response.get("result").expect("should have result");
        assert_eq!(
            result.get("value").and_then(|v| v.as_str()),
            Some("[1] 2"),
            "should capture printed value"
        );
        assert!(
            result.get("error").is_none() || result.get("error").and_then(|v| v.as_str()).is_none(),
            "should have no error"
        );

        // Test 2: stdout capture via cat()
        let response = send_ipc_request(
            &socket_path,
            "evaluate",
            serde_json::json!({ "code": "cat('hello_stdout\\n')" }),
        )
        .expect("evaluate should succeed");

        let result = response.get("result").expect("should have result");
        assert!(
            result
                .get("stdout")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("hello_stdout")),
            "should capture stdout from cat(): {result:?}"
        );

        // Test 3: Error capture
        let response = send_ipc_request(
            &socket_path,
            "evaluate",
            serde_json::json!({ "code": "stop('test_error_msg')" }),
        )
        .expect("evaluate should succeed");

        let result = response.get("result").expect("should have result");
        assert!(
            result
                .get("error")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("test_error_msg")),
            "should capture error message: {result:?}"
        );

        // Test 4: Mixed stdout + value
        let response = send_ipc_request(
            &socket_path,
            "evaluate",
            serde_json::json!({ "code": "cat('before\\n'); 42" }),
        )
        .expect("evaluate should succeed");

        let result = response.get("result").expect("should have result");
        assert!(
            result
                .get("stdout")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("before")),
            "should capture stdout: {result:?}"
        );
        assert_eq!(
            result.get("value").and_then(|v| v.as_str()),
            Some("[1] 42"),
            "should capture value: {result:?}"
        );

        terminal.quit().expect("Should quit cleanly");
    }

    /// Test that `visible=true` injects code into the REPL and captures output.
    ///
    /// This verifies the "blocking send" behavior:
    /// 1. Code is injected into the REPL prompt (like user_input/send)
    /// 2. R evaluates it normally, output appears in the terminal
    /// 3. IPC client blocks until evaluation completes
    /// 4. Response contains captured stdout/stderr from WriteConsoleEx
    #[test]
    fn test_ipc_evaluate_visible() {
        let (mut terminal, socket_path) = spawn_ipc_session();

        // Clear output buffer so we can detect new output
        terminal.clear_buffer().expect("clear buffer");

        // Evaluate with visible=true — code should appear at the prompt
        let response = send_ipc_request(
            &socket_path,
            "evaluate",
            serde_json::json!({ "code": "cat('visible_marker\\n'); 99", "visible": true }),
        )
        .expect("evaluate should succeed");

        // Verify the response has captured data
        let result = response.get("result").expect("should have result");
        // In visible mode, all output (including auto-printed value) is in stdout
        let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            stdout.contains("visible_marker"),
            "response stdout should contain cat() output: {result:?}"
        );
        assert!(
            stdout.contains("[1] 99"),
            "response stdout should contain auto-printed value: {result:?}"
        );
        // Structured value/error are not available in visible mode
        assert!(
            result.get("value").is_none() || result.get("value").and_then(|v| v.as_str()).is_none(),
            "visible mode should not have structured value: {result:?}"
        );

        // Verify the output appeared in the REPL terminal
        terminal
            .expect("visible_marker")
            .expect("stdout should appear in REPL terminal with visible=true");
        terminal
            .expect("[1] 99")
            .expect("value should appear in REPL terminal with visible=true");

        // Verify REPL returns to prompt after visible evaluate
        terminal
            .wait_for_prompt()
            .expect("Should return to prompt after visible evaluate");

        terminal.quit().expect("Should quit cleanly");
    }

    /// Test that `visible=false` (default) does NOT show output in the REPL terminal.
    #[test]
    fn test_ipc_evaluate_silent() {
        let (mut terminal, socket_path) = spawn_ipc_session();

        // Clear buffer
        terminal.clear_buffer().expect("clear buffer");

        // Evaluate with visible=false (default)
        let response = send_ipc_request(
            &socket_path,
            "evaluate",
            serde_json::json!({ "code": "cat('silent_marker')" }),
        )
        .expect("evaluate should succeed");

        let result = response.get("result").expect("should have result");
        assert!(
            result
                .get("stdout")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("silent_marker")),
            "response should contain stdout even in silent mode: {result:?}"
        );

        // Wait a bit and verify the output did NOT appear in the terminal
        std::thread::sleep(Duration::from_millis(500));
        let output = terminal.get_output().expect("get output");
        assert!(
            !output.contains("silent_marker"),
            "stdout should NOT appear in terminal with visible=false, but got: {output}"
        );

        terminal.quit().expect("Should quit cleanly");
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
        let socket_path = find_socket_path(terminal.process_id(), Duration::from_secs(10))
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
