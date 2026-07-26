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

    /// Query the interactive session's history for a command marker.
    fn query_history(socket_path: &str, marker: &str) -> serde_json::Value {
        let response = send_ipc_request(
            socket_path,
            "history",
            serde_json::json!({
                "all_sessions": true,
                "grep": marker,
                "limit": 10
            }),
        )
        .expect("history request should succeed");
        response
            .get("result")
            .cloned()
            .expect("history response should have a result")
    }

    /// Assert that a user_input command has been persisted with the expected
    /// session metadata, rather than only being evaluated in R.
    fn assert_ipc_history_entry(socket_path: &str, marker: &str) {
        let result = query_history(socket_path, marker);
        let entries = result
            .get("entries")
            .and_then(|entries| entries.as_array())
            .expect("history result should contain entries");
        let entry = entries
            .iter()
            .find(|entry| entry.get("command").and_then(|v| v.as_str()) == Some(marker))
            .unwrap_or_else(|| panic!("history should contain {marker}: {result}"));

        assert!(entry.get("timestamp").and_then(|v| v.as_str()).is_some());
        assert!(entry.get("cwd").and_then(|v| v.as_str()).is_some());
        assert!(entry.get("session_id").and_then(|v| v.as_i64()).is_some());
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

        // Send a visible command as a sentinel to prove the terminal is caught up
        send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": "cat('sentinel_after_silent')" }),
        )
        .expect("sentinel user_input should succeed");

        // Wait for the sentinel to appear in terminal output
        terminal
            .expect("sentinel_after_silent")
            .expect("sentinel should appear in terminal");

        // Now check that the silent_marker never appeared in the terminal
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

    /// Test that an IPC request arriving while reedline is already waiting
    /// persists the user_input command through the ExternalBreak path.
    #[test]
    fn test_ipc_user_input_history_external_break() {
        let tmp = tempfile::TempDir::new().expect("create history dir");
        let history_dir = tmp.path().to_str().expect("history dir should be UTF-8");
        let (mut terminal, socket_path) = {
            let mut terminal =
                Terminal::spawn_with_args(&["--with-ipc", "--history-dir", history_dir])
                    .expect("Failed to spawn arf with IPC");
            terminal
                .wait_for_prompt()
                .expect("Should show prompt after startup");
            std::thread::sleep(Duration::from_millis(500));
            let socket_path = find_socket_path(terminal.process_id(), Duration::from_secs(1))
                .expect("find socket");
            (terminal, socket_path)
        };

        let marker = "ipc_external_history_marker <- 1";
        let response = send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": marker }),
        )
        .expect("IPC request should succeed");
        assert_eq!(response["result"]["accepted"], true);
        terminal
            .wait_for_prompt()
            .expect("Should return to prompt after IPC input");

        assert_ipc_history_entry(&socket_path, marker);
        terminal.quit().expect("Should quit cleanly");
    }

    /// Test the fast-path timing window by sending immediately after the
    /// prompt is displayed, before reedline has necessarily entered its input
    /// loop. The same assertion also protects against route-dependent saves.
    #[test]
    fn test_ipc_user_input_history_fast_path() {
        let tmp = tempfile::TempDir::new().expect("create history dir");
        let history_dir = tmp.path().to_str().expect("history dir should be UTF-8");
        let mut terminal = Terminal::spawn_with_args(&["--with-ipc", "--history-dir", history_dir])
            .expect("Failed to spawn arf with IPC");
        terminal
            .wait_for_prompt()
            .expect("Should show prompt after startup");
        let socket_path = find_socket_path(terminal.process_id(), Duration::from_secs(10))
            .expect("Should find IPC socket path");

        let marker = "ipc_fast_history_marker <- 1";
        let response = send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": marker }),
        )
        .expect("IPC request should succeed");
        assert_eq!(response["result"]["accepted"], true);
        terminal
            .wait_for_prompt()
            .expect("Should return to prompt after IPC input");

        assert_ipc_history_entry(&socket_path, marker);
        terminal.quit().expect("Should quit cleanly");
    }

    /// Test that rejecting IPC user_input does not advance the prompt line.
    ///
    /// When the user has typed something in the buffer, IPC user_input should
    /// be rejected with USER_IS_TYPING. After rejection, the prompt must stay
    /// on the same row — no extra blank line should appear.
    #[test]
    fn test_ipc_user_input_reject_no_extra_line() {
        let (mut terminal, socket_path) = spawn_ipc_session();

        // Record the prompt row before sending any input
        let prompt_row_before = terminal
            .cursor_position()
            .expect("should get cursor position")
            .0;

        // Type something into the buffer (without pressing Enter)
        terminal.send("hello").expect("send text");

        // Give reedline time to process the keystrokes
        std::thread::sleep(Duration::from_millis(500));

        // Send IPC user_input — should be rejected because buffer is non-empty
        let response = send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": "1 + 1" }),
        )
        .expect("IPC request should succeed");

        // Verify rejection
        let error = response.get("error").expect("should have error");
        assert_eq!(
            error.get("code").and_then(|c| c.as_i64()),
            Some(-32003),
            "should be USER_IS_TYPING error, got: {response:?}"
        );

        // Give the terminal time to process the ExternalBreak → continue cycle
        std::thread::sleep(Duration::from_millis(500));

        // The prompt row should NOT have advanced
        let prompt_row_after = terminal
            .cursor_position()
            .expect("should get cursor position")
            .0;
        assert_eq!(
            prompt_row_before, prompt_row_after,
            "prompt row should not advance after rejected IPC (before={}, after={})",
            prompt_row_before, prompt_row_after
        );

        // Verify the user's typed text is still visible on the current line
        terminal
            .current_line()
            .assert_contains("hello")
            .expect("user's typed text should still be visible");

        terminal.quit().expect("Should quit cleanly");
    }

    /// Test that IPC user_input does not create an extra blank prompt line.
    ///
    /// After accepting and executing an IPC user_input, the next prompt should
    /// appear immediately after the R output — no extra blank line in between.
    #[test]
    fn test_ipc_user_input_no_extra_blank_line() {
        let (mut terminal, socket_path) = spawn_ipc_session();

        // Send IPC user_input with a command that produces known output
        let response = send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": "cat('marker_output\\n')" }),
        )
        .expect("IPC request should succeed");

        assert!(
            response
                .get("result")
                .and_then(|r| r.get("accepted"))
                .and_then(|a| a.as_bool())
                == Some(true),
            "user_input should be accepted, got: {response:?}"
        );

        // Wait for output and next prompt
        terminal
            .expect("marker_output")
            .expect("should see R output");
        terminal.wait_for_prompt().expect("should return to prompt");

        let screen = terminal.screen().expect("should get terminal screen");
        assert!(
            screen
                .lines
                .iter()
                .any(|line| line.contains("agent> cat('marker_output\\n')")),
            "IPC echo should remain visible when the expression produces output; screen:\n{}",
            screen.lines.join("\n")
        );

        // The line immediately above the prompt should be the R output,
        // not a blank line.
        terminal
            .previous_line(1)
            .assert_contains("marker_output")
            .expect("line above prompt should be R output, not a blank line");

        terminal.quit().expect("Should quit cleanly");
    }

    /// Test that an IPC user_input echo remains visible when R produces no output.
    #[test]
    fn test_ipc_user_input_silent_expression_keeps_echo() {
        let (mut terminal, socket_path) = spawn_ipc_session();

        let response = send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": "ipc_silent_echo <- 1" }),
        )
        .expect("IPC request should succeed");

        assert!(
            response
                .get("result")
                .and_then(|r| r.get("accepted"))
                .and_then(|a| a.as_bool())
                == Some(true),
            "user_input should be accepted, got: {response:?}"
        );

        // The IPC response is sent as soon as the input is accepted, before R
        // evaluates it. `expect()` matches against the whole accumulated
        // buffer, so the echo and the post-evaluation prompt can arrive in
        // the same PTY read — clearing the buffer between two separate
        // `expect()` calls would risk discarding the prompt bytes and
        // timing out. Instead, match a single regex requiring a "> " prompt
        // AFTER the echoed code; the only such prompt is the real one
        // redrawn once evaluation completes (the "agent> " echo itself
        // precedes the code, not follows it). A fixed sleep would let the
        // test pass by inspecting the echo before the repaint that used to
        // erase it — this waits for a definitive post-evaluation signal
        // instead.
        terminal
            .expect_regex(r"ipc_silent_echo <- 1[\s\S]*> ")
            .expect("should return to a fresh prompt after evaluation");

        let screen = terminal.screen().expect("should get terminal screen");
        assert!(
            screen
                .lines
                .iter()
                .any(|line| line.contains("agent> ipc_silent_echo <- 1")),
            "IPC echo should remain visible after a silent expression; screen:\n{}",
            screen.lines.join("\n")
        );

        terminal.quit().expect("Should quit cleanly");
    }

    /// Test that --with-ipc --ipc-bind honours the custom socket path.
    ///
    /// The session must be reachable directly at the specified path without
    /// going through session discovery.
    #[test]
    fn test_with_ipc_bind_custom_socket() {
        let tmp = tempfile::TempDir::new().expect("create temp dir");
        let sock_path = tmp.path().join("custom.sock");
        let sock_str = sock_path.display().to_string();

        let mut terminal = Terminal::spawn_with_args(&["--with-ipc", "--ipc-bind", &sock_str])
            .expect("Failed to spawn arf with --with-ipc --ipc-bind");

        terminal
            .wait_for_prompt()
            .expect("Should show prompt after startup");

        // Socket must exist at the custom path (no discovery needed)
        assert!(
            sock_path.exists(),
            "custom socket should exist at: {sock_str}"
        );

        // IPC must work via the custom socket path directly
        let response = send_ipc_request(
            &sock_str,
            "evaluate",
            serde_json::json!({ "code": "1 + 1" }),
        )
        .expect("IPC request via custom socket should succeed");

        let result = response.get("result").expect("should have result");
        assert_eq!(
            result.get("value").and_then(|v| v.as_str()),
            Some("[1] 2"),
            "should return correct result: {result:?}"
        );

        terminal.quit().expect("Should quit cleanly");
    }

    /// Test that --with-ipc --ipc-pid-file writes the PID on startup and
    /// removes the file when the REPL exits.
    #[test]
    fn test_with_ipc_pid_file_lifecycle() {
        let tmp = tempfile::TempDir::new().expect("create temp dir");
        let pid_path = tmp.path().join("arf.pid");
        let pid_str = pid_path.display().to_string();

        let mut terminal = Terminal::spawn_with_args(&["--with-ipc", "--ipc-pid-file", &pid_str])
            .expect("Failed to spawn arf with --with-ipc --ipc-pid-file");

        terminal
            .wait_for_prompt()
            .expect("Should show prompt after startup");

        // PID file must be written before the first prompt
        assert!(
            pid_path.exists(),
            "PID file should exist after startup: {pid_str}"
        );

        let pid_content = std::fs::read_to_string(&pid_path).expect("should read PID file");
        let expected_pid = terminal
            .process_id()
            .expect("should have process ID")
            .to_string();
        assert_eq!(
            pid_content.trim(),
            expected_pid,
            "PID file should contain the process PID"
        );

        // Exit via q() and wait for the process to terminate
        terminal.send_line("q()").expect("send q()");
        terminal
            .wait_for_exit(Duration::from_secs(10))
            .expect("Process should exit after q()");

        // PID file must be removed on exit (via atexit handler)
        assert!(
            !pid_path.exists(),
            "PID file should be removed after exit: {pid_str}"
        );
    }

    /// Test that --with-ipc --ipc-pid-file works with Ctrl+D exit.
    #[test]
    fn test_with_ipc_pid_file_ctrld_cleanup() {
        let tmp = tempfile::TempDir::new().expect("create temp dir");
        let pid_path = tmp.path().join("arf.pid");
        let pid_str = pid_path.display().to_string();

        let mut terminal = Terminal::spawn_with_args(&["--with-ipc", "--ipc-pid-file", &pid_str])
            .expect("Failed to spawn arf with --with-ipc --ipc-pid-file");

        terminal
            .wait_for_prompt()
            .expect("Should show prompt after startup");

        assert!(pid_path.exists(), "PID file should exist: {pid_str}");

        // Exit via Ctrl+D
        terminal.send_eof().expect("send Ctrl+D");
        terminal
            .wait_for_exit(Duration::from_secs(10))
            .expect("Process should exit after Ctrl+D");

        assert!(
            !pid_path.exists(),
            "PID file should be removed after Ctrl+D exit: {pid_str}"
        );
    }

    /// Test that `arf ipc eval` without a code argument exits with code 2
    /// and emits a structured JSON error when stdin is a TTY.
    ///
    /// This test uses a PTY so that `is_terminal()` on stdin returns true,
    /// which triggers the NO_CODE_PROVIDED error path.
    #[test]
    fn test_ipc_eval_no_code_tty_error() {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        use std::io::Read;

        let sessions_dir = tempfile::tempdir().expect("Failed to create temp sessions dir");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("Failed to open PTY");

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_arf"));
        cmd.args(["ipc", "eval"]);
        cmd.env("ARF_IPC_SESSIONS_DIR", sessions_dir.path());

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .expect("Failed to spawn arf ipc eval");

        // Drop slave so the child's stdin/stdout/stderr are connected to the PTY only.
        // Don't write anything — stdin is a TTY, so NO_CODE_PROVIDED fires immediately.
        drop(pair.slave);

        // Read all PTY output (stdout and stderr are merged on the PTY master).
        let mut output = String::new();
        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        // EIO (errno=5) is expected on Unix when the child exits and the PTY closes.
        if let Err(e) = reader.read_to_string(&mut output) {
            assert_eq!(e.raw_os_error(), Some(5), "unexpected PTY read error: {e}");
        }

        let status = child.wait().expect("Failed to wait for child");
        assert_eq!(
            status.exit_code(),
            2,
            "should exit with code 2 (client-side failure): output={output}"
        );

        // Extract and parse the JSON error from the PTY output.
        // PTY merges stdout+stderr; slice from first '{' to last '}' to handle
        // any ANSI escape sequences or control characters before/after the JSON.
        let json_start = output
            .find('{')
            .unwrap_or_else(|| panic!("no JSON object found in PTY output: {output}"));
        let json_end = output
            .rfind('}')
            .unwrap_or_else(|| panic!("no closing '}}' found in PTY output: {output}"));
        let json_str = &output[json_start..=json_end];
        let json: serde_json::Value = serde_json::from_str(json_str)
            .unwrap_or_else(|e| panic!("PTY output is not valid JSON: {e}\noutput: {output}"));
        assert_eq!(
            json["error"]["code"].as_str(),
            Some("NO_CODE_PROVIDED"),
            "error.code should be NO_CODE_PROVIDED: {json}"
        );
        assert!(
            json["error"]["message"].as_str().is_some(),
            "error.message should be present: {json}"
        );
        assert!(
            json["error"]["hint"].as_str().is_some(),
            "error.hint should be present: {json}"
        );
    }
}
