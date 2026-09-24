//! Cross-platform IPC integration tests for arf.
//!
//! These tests verify IPC functionality without relying on terminal output
//! verification, making them runnable on both Unix and Windows.
//!
//! These tests complement `tui_tests.rs`: the TUI cases verify interactive
//! screen and prompt behavior, while this file keeps low-level JSON-RPC and
//! transport coverage independent of those assertions.
//!
//! These tests verify JSON-RPC responses over platform-aware transport (Unix
//! sockets / Windows named pipes). TUI state is used only to synchronize
//! approval prompts for interactive requests.
//!
//! Each test spawns a fresh arf process with an isolated IPC session directory.

#[cfg(unix)]
use std::io::Read;
use std::io::Write;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use std::path::Path;
use tempfile::TempDir;
use tui_test::{AutomaticRecording, OpenOptions, Operation, OperationResult, RunOptions, Session};

/// Timeout for waiting for IPC server to start.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_TIMEOUT: Duration = Duration::from_secs(180);

/// Timeout for IPC request/response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Minimal process wrapper for cross-platform IPC testing.
///
/// Spawns arf in a tui-test session with `--with-ipc` and waits for the IPC
/// server to become connectable. Application output is only observed when a
/// test needs to approve an interactive request.
struct IpcTestProcess {
    session: Session,
    socket_path: String,
    _sessions_dir: TempDir,
    _watchdog_done: mpsc::Sender<()>,
}

impl IpcTestProcess {
    /// Spawn arf with `--with-ipc` and wait for IPC server to be ready.
    fn spawn() -> Result<Self, String> {
        let (watchdog_done, watchdog) = mpsc::channel();
        thread::spawn(move || {
            if matches!(
                watchdog.recv_timeout(TEST_TIMEOUT),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                let message = "tui-test IPC session exceeded 180s; terminating test process\n";
                let _ = std::io::stderr().lock().write_all(message.as_bytes());
                std::process::exit(1);
            }
        });

        let session = Session::new("arf-ipc-tests");
        let sessions_dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let defaults = OpenOptions::default();
        let opened = session
            .run(RunOptions {
                backend: defaults.backend,
                program: env!("CARGO_BIN_EXE_arf").into(),
                args: [
                    "--no-history",
                    "--with-ipc",
                    // Keep arbitrary-expression coverage separate from policy tests.
                    "--ipc-eval-unrestricted",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                profile: defaults.profile,
                cols: 80,
                rows: 24,
                cwd: None,
                env: vec![(
                    "ARF_IPC_SESSIONS_DIR".to_owned(),
                    sessions_dir.path().to_string_lossy().into_owned(),
                )],
                wait_ready: Some(false),
                restart: false,
                timeouts: defaults.timeouts,
                recording: AutomaticRecording::default(),
            })
            .map_err(|e| format!("Failed to spawn arf: {e}"))?;

        // Wait for session metadata to appear (indicates IPC server startup).
        let socket_path =
            match find_socket_path(opened.shell_pid, sessions_dir.path(), STARTUP_TIMEOUT) {
                Some(path) => path,
                None => {
                    let _ = session.close();
                    return Err("IPC server did not start within timeout".into());
                }
            };

        Ok(IpcTestProcess {
            session,
            socket_path,
            _sessions_dir: sessions_dir,
            _watchdog_done: watchdog_done,
        })
    }

    /// Send a JSON-RPC request and return the response.
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        send_ipc_request(&self.socket_path, method, params)
    }

    /// Wait for text to appear on the emulated screen.
    fn wait_for_screen_text(&self, expected: &str) -> Result<(), String> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        // tui-test's screen text omits trailing blank cells from each row.
        let expected = expected.trim_end();
        loop {
            let state = match self.session.execute(Operation::State) {
                Ok(OperationResult::State(state)) => *state,
                Ok(_) => return Err("tui-test returned an unexpected state response".into()),
                Err(error) => return Err(error.to_string()),
            };
            if state.text.contains(expected) {
                return Ok(());
            }
            if state.exited.is_some() {
                return Err(format!(
                    "arf exited waiting for screen text '{expected}': {state:?}"
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Timed out waiting for screen text '{expected}'. Current emulated screen state:\n{state:?}"
                ));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// Send a line of terminal input through tui-test's portable input path.
    fn submit(&self, text: &str) -> Result<(), String> {
        self.session
            .execute(Operation::Submit {
                data: Some(text.to_owned()),
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn close(&self) {
        let _ = self.submit("q()");
        let _ = self.session.execute(Operation::WaitExit {
            timeout_ms: Some(500),
        });
        let _ = self.session.close();
    }
}

impl Drop for IpcTestProcess {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Socket/pipe discovery
// ---------------------------------------------------------------------------

/// Find the IPC socket path by scanning session files.
/// Retries until a connectable session appears or timeout is reached.
fn find_socket_path(pid: Option<u32>, sessions_dir: &Path, timeout: Duration) -> Option<String> {
    let start = Instant::now();

    while start.elapsed() < timeout {
        if let Ok(entries) = std::fs::read_dir(sessions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "json")
                    && let Ok(contents) = std::fs::read_to_string(&path)
                    && let Ok(info) = serde_json::from_str::<serde_json::Value>(&contents)
                {
                    if let Some(target_pid) = pid
                        && info.get("pid").and_then(|v| v.as_u64()) != Some(u64::from(target_pid))
                    {
                        continue;
                    }
                    if let Some(socket) = info.get("socket_path").and_then(|v| v.as_str()) {
                        #[cfg(unix)]
                        let ready = is_connectable(socket);
                        #[cfg(windows)]
                        // The server writes metadata only after creating its
                        // first pipe instance. Opening it as a probe consumes
                        // that instance and races the real client.
                        let ready = true;
                        if ready {
                            return Some(socket.to_string());
                        }
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    None
}

/// Check whether the Unix socket is accepting connections.
#[cfg(unix)]
fn is_connectable(socket_path: &str) -> bool {
    std::os::unix::net::UnixStream::connect(socket_path).is_ok()
}

// ---------------------------------------------------------------------------
// IPC transport (platform-specific)
// ---------------------------------------------------------------------------

/// Send a JSON-RPC request and return the parsed response.
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

    #[cfg(unix)]
    {
        send_ipc_request_unix(socket_path, &body)
    }

    #[cfg(windows)]
    {
        send_ipc_request_windows(socket_path, &body)
    }
}

/// Send via Unix socket with HTTP wrapping.
///
/// The response parser is intentionally simplistic: it reads everything after
/// `\r\n\r\n` as JSON. This works because we send `Connection: close` and the
/// server closes the connection after responding.
#[cfg(unix)]
fn send_ipc_request_unix(socket_path: &str, body: &str) -> Result<serde_json::Value, String> {
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

    let mut stream =
        UnixStream::connect(socket_path).map_err(|e| format!("Connect failed: {e}"))?;
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
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

#[cfg(windows)]
fn send_ipc_request_windows(socket_path: &str, body: &str) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;

    let socket_path = socket_path.to_string();
    let body = body.to_string();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

    rt.block_on(async {
        let mut pipe = ClientOptions::new()
            .open(&socket_path)
            .map_err(|e| format!("Connect failed: {e}"))?;

        pipe.write_all(body.as_bytes())
            .await
            .map_err(|e| format!("Write failed: {e}"))?;
        pipe.flush()
            .await
            .map_err(|e| format!("Flush failed: {e}"))?;

        let mut response_buf = Vec::new();
        match tokio::time::timeout(REQUEST_TIMEOUT, pipe.read_to_end(&mut response_buf)).await {
            Ok(result) => result.map_err(|e| format!("Read failed: {e}"))?,
            Err(_) => return Err("Request timed out".to_string()),
        };

        let text = String::from_utf8_lossy(&response_buf);
        let json_body = if let Some(pos) = text.find("\r\n\r\n") {
            &text[pos + 4..]
        } else {
            &text
        };

        serde_json::from_str(json_body).map_err(|e| format!("Parse failed: {e}: {json_body}"))
    })
}

// ===========================================================================
// Tests
// ===========================================================================

/// Test that IPC `evaluate` captures a visible R value.
#[test]
fn test_ipc_evaluate_value() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let response = process
        .request("evaluate", serde_json::json!({ "code": "1 + 1" }))
        .expect("evaluate should succeed");

    let result = response.get("result").expect("should have result");
    assert_eq!(
        result.get("value").and_then(|v| v.as_str()),
        Some("[1] 2"),
        "should capture printed value"
    );
    assert!(
        result.get("error").is_none() || result.get("error").unwrap().is_null(),
        "should have no error"
    );
}

/// Test that IPC `evaluate` captures stdout from `cat()`.
#[test]
fn test_ipc_evaluate_stdout() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let response = process
        .request(
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
}

/// Test that IPC `evaluate` captures R errors via `tryCatch`.
#[test]
fn test_ipc_evaluate_error() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let response = process
        .request(
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
}

/// Test that IPC `evaluate` captures both stdout and value in a mixed expression.
#[test]
fn test_ipc_evaluate_mixed() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let response = process
        .request(
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
}

/// Test that `visible=true` evaluate returns captured output.
///
/// This transport-only test verifies the JSON-RPC response; the corresponding
/// tui-test case additionally verifies terminal output and prompt completion.
#[test]
fn test_ipc_evaluate_visible() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let socket_path = process.socket_path.clone();
    let request = thread::spawn(move || {
        send_ipc_request(
            &socket_path,
            "evaluate",
            serde_json::json!({
                "code": r#"cat("vis_marker\n"); 99"#,
                "visible": true
            }),
        )
    });
    process
        .wait_for_screen_text("Press y to approve, any other key declines: ")
        .expect("approval prompt should appear on the emulated screen");
    process.submit("y").expect("approve visible evaluate");
    let response = request
        .join()
        .expect("request thread should not panic")
        .expect("visible evaluate should succeed");

    let result = response.get("result").expect("should have result");
    let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        stdout.contains("vis_marker"),
        "visible eval should capture stdout: {result:?}"
    );
    assert!(
        stdout.contains("[1] 99"),
        "visible eval should capture auto-printed value in stdout: {result:?}"
    );
    // Structured value/error fields are not available in visible mode
    assert!(
        result.get("value").is_none() || result.get("value").unwrap().is_null(),
        "visible mode should not have structured value: {result:?}"
    );
}

/// Test that IPC `user_input` is accepted when R is at the prompt.
#[test]
fn test_ipc_user_input() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let socket_path = process.socket_path.clone();
    let request = thread::spawn(move || {
        send_ipc_request(
            &socket_path,
            "user_input",
            serde_json::json!({ "code": "cat('ipc_input_test')" }),
        )
    });
    process
        .wait_for_screen_text("Press y to approve, any other key declines: ")
        .expect("approval prompt should appear on the emulated screen");
    process.submit("y").expect("approve user_input");
    let response = request
        .join()
        .expect("request thread should not panic")
        .expect("user_input should succeed");

    assert!(
        response
            .get("result")
            .and_then(|r| r.get("accepted"))
            .and_then(|a| a.as_bool())
            == Some(true),
        "user_input should be accepted: {response:?}"
    );
}

/// Test that `shutdown` is rejected in REPL mode (only available in headless).
#[test]
fn test_ipc_shutdown_rejected_in_repl() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let response = process
        .request("shutdown", serde_json::json!({}))
        .expect("shutdown request should get a response");

    let error = response
        .get("error")
        .expect("should have error for shutdown in REPL mode");
    assert_eq!(
        error.get("code").and_then(|v| v.as_i64()),
        Some(-32601), // METHOD_NOT_FOUND
        "should return METHOD_NOT_FOUND: {error:?}"
    );
}

/// Test that sequential evaluations work correctly (no stale state).
#[test]
fn test_ipc_evaluate_sequential() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    // First evaluation
    let r1 = process
        .request("evaluate", serde_json::json!({ "code": "x <- 123" }))
        .expect("first eval should succeed");
    assert!(r1.get("result").is_some(), "first eval should have result");

    // Second evaluation uses result of first
    let r2 = process
        .request("evaluate", serde_json::json!({ "code": "x + 1" }))
        .expect("second eval should succeed");
    let result = r2.get("result").expect("should have result");
    assert_eq!(
        result.get("value").and_then(|v| v.as_str()),
        Some("[1] 124"),
        "second eval should see variable from first: {result:?}"
    );
}
