//! Cross-platform IPC integration tests for arf.
//!
//! These tests verify IPC functionality without relying on terminal output
//! verification, making them runnable on both Unix and Windows.
//!
//! Key differences from `pty_ipc_tests.rs`:
//! - No terminal output assertions (no vt100 screen parsing)
//! - Platform-aware transport (Unix sockets / Windows named pipes)
//! - Only JSON-RPC responses are verified
//!
//! Each test spawns a fresh arf process. Run with `--test-threads=1` to avoid
//! resource contention from multiple R processes starting simultaneously.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Timeout for waiting for IPC server to start.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for IPC request/response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Minimal process wrapper for cross-platform IPC testing.
///
/// Spawns arf in a PTY (required by reedline) with `--with-ipc` and waits
/// for the IPC server to become connectable. Does not parse terminal output.
struct IpcTestProcess {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    shutdown: Arc<AtomicBool>,
    _reader_handle: Option<thread::JoinHandle<()>>,
    socket_path: String,
}

impl IpcTestProcess {
    /// Spawn arf with `--with-ipc` and wait for IPC server to be ready.
    fn spawn() -> Result<Self, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {e}"))?;

        let mut cmd = CommandBuilder::new(bin_path);
        cmd.arg("--no-history");
        cmd.arg("--with-ipc");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn arf: {e}"))?;

        let pty_writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to get PTY writer: {e}"))?;
        let mut pty_reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to get PTY reader: {e}"))?;

        drop(pair.slave);

        let pty_writer = Arc::new(Mutex::new(pty_writer));
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        // On Unix, reedline sends CSI 6n (cursor position query) via the PTY.
        // We must respond or crossterm blocks with a timeout, slowing startup.
        // On Windows, crossterm uses WinAPI for cursor position — no query needed.
        #[cfg(unix)]
        let pty_writer_clone = Arc::clone(&pty_writer);

        let reader_handle = thread::spawn(move || {
            #[cfg(unix)]
            {
                // Use vt100 parser to reliably detect CSI 6n across read boundaries.
                let (query_tx, query_rx) = std::sync::mpsc::channel::<()>();

                struct QueryDetector {
                    tx: std::sync::mpsc::Sender<()>,
                }
                impl vt100::Callbacks for QueryDetector {
                    fn unhandled_csi(
                        &mut self,
                        _screen: &mut vt100::Screen,
                        _prefix: Option<u8>,
                        _intermediate: Option<u8>,
                        params: &[&[u16]],
                        c: char,
                    ) {
                        if c == 'n'
                            && (params.is_empty()
                                || (params.len() == 1 && params[0].len() == 1 && params[0][0] == 6))
                        {
                            let _ = self.tx.send(());
                        }
                    }
                }

                let callbacks = QueryDetector { tx: query_tx };
                let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, callbacks);
                let mut buf = [0u8; 4096];

                loop {
                    if shutdown_clone.load(Ordering::Relaxed) {
                        break;
                    }
                    match pty_reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            parser.process(&buf[..n]);
                            // Respond to any cursor queries detected
                            while query_rx.try_recv().is_ok() {
                                let response = b"\x1b[1;1R";
                                if let Ok(mut writer) = pty_writer_clone.lock() {
                                    let _ = writer.write_all(response);
                                    let _ = writer.flush();
                                }
                            }
                        }
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::WouldBlock
                                && e.kind() != std::io::ErrorKind::Interrupted
                            {
                                break;
                            }
                        }
                    }
                }
            }

            #[cfg(not(unix))]
            {
                // On Windows, just consume PTY output to prevent buffer fill-up.
                let mut buf = [0u8; 4096];
                loop {
                    if shutdown_clone.load(Ordering::Relaxed) {
                        break;
                    }
                    match pty_reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::WouldBlock
                                && e.kind() != std::io::ErrorKind::Interrupted
                            {
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Wait for session file to appear (indicates IPC server is ready)
        let pid = child.process_id();
        let socket_path = find_socket_path(pid, STARTUP_TIMEOUT)
            .ok_or("IPC server did not start within timeout")?;

        Ok(IpcTestProcess {
            child,
            _pty_writer: pty_writer,
            shutdown,
            _reader_handle: Some(reader_handle),
            socket_path,
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
}

impl Drop for IpcTestProcess {
    fn drop(&mut self) {
        // Signal the reader thread to stop first, so it can exit during the
        // grace period rather than remaining blocked on pty_reader.read().
        self.shutdown.store(true, Ordering::Relaxed);

        // Send q() to trigger clean shutdown (session file cleanup, etc.)
        if let Ok(mut writer) = self._pty_writer.lock() {
            let _ = writer.write_all(b"q()\n");
            let _ = writer.flush();
        }
        // Give it a moment to shut down cleanly
        thread::sleep(Duration::from_millis(500));
        let _ = self.child.kill();

        // Intentionally do NOT join the reader thread — it may be permanently
        // blocked on pty_reader.read() after child kill, which would hang Drop
        // (and thus the entire test run). Leaking the thread is acceptable for
        // tests; the OS reclaims it on process exit.
        let _ = self._reader_handle.take();
    }
}

// ---------------------------------------------------------------------------
// Socket/pipe discovery
// ---------------------------------------------------------------------------

/// Find the IPC socket path by scanning session files.
/// Retries until a connectable session appears or timeout is reached.
///
/// When `pid` is `None` (e.g., platform can't retrieve child PID), this
/// connects to any available session. In parallel test runs this could cause
/// cross-talk; use `--test-threads=1` to avoid this.
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
                    if let Some(target_pid) = pid
                        && info.get("pid").and_then(|v| v.as_u64()) != Some(u64::from(target_pid))
                    {
                        continue;
                    }
                    if let Some(socket) = info.get("socket_path").and_then(|v| v.as_str())
                        && is_connectable(socket)
                    {
                        return Some(socket.to_string());
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    None
}

/// Check if a socket/pipe is connectable.
#[cfg(unix)]
fn is_connectable(socket_path: &str) -> bool {
    std::os::unix::net::UnixStream::connect(socket_path).is_ok()
}

#[cfg(windows)]
fn is_connectable(socket_path: &str) -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(socket_path)
        .is_ok()
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

// On Windows, crossterm's cursor::position() uses WinAPI which doesn't work
// inside ConPTY. This prevents reedline from initializing, so arf never
// reaches the prompt and the IPC server never starts. True Windows IPC
// testing requires a headless mode (no reedline, just R + IPC server).

/// Test that IPC `evaluate` captures a visible R value.
#[test]
#[cfg_attr(windows, ignore = "ConPTY cursor position incompatibility")]
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
#[cfg_attr(windows, ignore = "ConPTY cursor position incompatibility")]
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
#[cfg_attr(windows, ignore = "ConPTY cursor position incompatibility")]
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
#[cfg_attr(windows, ignore = "ConPTY cursor position incompatibility")]
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
/// Unlike pty_ipc_tests, we cannot verify the output appeared in the terminal.
/// We only verify the JSON-RPC response contains the expected captured data.
#[test]
#[cfg_attr(windows, ignore = "ConPTY cursor position incompatibility")]
fn test_ipc_evaluate_visible() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let response = process
        .request(
            "evaluate",
            serde_json::json!({ "code": "cat('vis_marker\\n'); 99", "visible": true }),
        )
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
#[cfg_attr(windows, ignore = "ConPTY cursor position incompatibility")]
fn test_ipc_user_input() {
    let process = IpcTestProcess::spawn().expect("Failed to spawn arf with IPC");

    let response = process
        .request(
            "user_input",
            serde_json::json!({ "code": "cat('ipc_input_test')" }),
        )
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

/// Test that sequential evaluations work correctly (no stale state).
#[test]
#[cfg_attr(windows, ignore = "ConPTY cursor position incompatibility")]
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
