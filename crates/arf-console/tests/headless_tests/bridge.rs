//! Stdin/stdout bridge integration tests for `arf ipc attach`.

use super::support::{HeadlessProcess, wait_for_history_until};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const BRIDGE_TIMEOUT: Duration = Duration::from_secs(10);

struct BridgeProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: mpsc::Receiver<Result<String, String>>,
    stderr: Arc<Mutex<String>>,
    stderr_done: mpsc::Receiver<()>,
    _unread_stdout: Option<ChildStdout>,
}

#[derive(Clone, Copy)]
enum StdoutMode {
    Read,
    Drop,
    Hold,
}

impl BridgeProcess {
    fn spawn(pid: u32, subscriptions: &[&str]) -> Self {
        Self::spawn_inner(pid, subscriptions, StdoutMode::Read, None)
    }

    fn spawn_with_sessions_dir(pid: u32, sessions_dir: &str) -> Self {
        Self::spawn_inner(pid, &[], StdoutMode::Read, Some(sessions_dir))
    }

    fn spawn_with_broken_stdout(pid: u32, subscriptions: &[&str]) -> Self {
        Self::spawn_inner(pid, subscriptions, StdoutMode::Drop, None)
    }

    fn spawn_with_blocked_stdout(pid: u32, subscriptions: &[&str]) -> Self {
        Self::spawn_inner(pid, subscriptions, StdoutMode::Hold, None)
    }

    fn spawn_inner(
        pid: u32,
        subscriptions: &[&str],
        stdout_mode: StdoutMode,
        sessions_dir: Option<&str>,
    ) -> Self {
        let bin_path = env!("CARGO_BIN_EXE_arf");
        let mut command = Command::new(bin_path);
        command
            .args(["ipc", "attach", "--pid", &pid.to_string()])
            .env_remove("ARF_CONFIG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(sessions_dir) = sessions_dir {
            command.env("ARF_IPC_SESSIONS_DIR", sessions_dir);
        }
        for subscription in subscriptions {
            command.args(["--subscribe", subscription]);
        }

        let mut child = command.spawn().expect("spawn arf ipc attach bridge");
        let stdin = child.stdin.take().expect("bridge stdin should be piped");
        let stdout = child.stdout.take().expect("bridge stdout should be piped");
        let stderr = child.stderr.take().expect("bridge stderr should be piped");
        let (stdout_tx, stdout_rx) = mpsc::channel();
        let unread_stdout = match stdout_mode {
            StdoutMode::Read => {
                thread::spawn(move || {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        if stdout_tx
                            .send(line.map_err(|error| error.to_string()))
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                None
            }
            // Closing the read end makes the bridge's first response write fail.
            StdoutMode::Drop => {
                drop(stdout);
                None
            }
            // Keep the read end alive but never consume data, applying pipe backpressure.
            StdoutMode::Hold => Some(stdout),
        };

        let stderr_output = Arc::new(Mutex::new(String::new()));
        let stderr_copy = Arc::clone(&stderr_output);
        let (stderr_done_tx, stderr_done_rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if let Ok(mut output) = stderr_copy.lock() {
                            output.push_str(&line);
                            output.push('\n');
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = stderr_done_tx.send(());
        });

        Self {
            child,
            stdin: Some(stdin),
            stdout: stdout_rx,
            stderr: stderr_output,
            stderr_done: stderr_done_rx,
            _unread_stdout: unread_stdout,
        }
    }

    fn send_line(&mut self, request: &str) {
        let stdin = self.stdin.as_mut().expect("bridge stdin is open");
        stdin
            .write_all(request.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .expect("write JSON-RPC request to bridge");
    }

    fn recv_json(&self) -> Value {
        let line = self
            .stdout
            .recv_timeout(BRIDGE_TIMEOUT)
            .expect("timed out waiting for bridge stdout")
            .expect("read bridge stdout");
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("bridge emitted invalid JSON: {error}: {line:?}"))
    }

    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait for bridge") {
                return status;
            }
            assert!(Instant::now() < deadline, "bridge did not exit in time");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn stderr_after_exit(&self) -> String {
        self.stderr_done
            .recv_timeout(BRIDGE_TIMEOUT)
            .expect("timed out draining bridge stderr");
        self.stderr.lock().expect("bridge stderr lock").clone()
    }

    fn stdout_after_exit(&self) -> String {
        let mut output = String::new();
        loop {
            match self.stdout.recv_timeout(BRIDGE_TIMEOUT) {
                Ok(Ok(line)) => {
                    output.push_str(&line);
                    output.push('\n');
                }
                Ok(Err(error)) => panic!("read bridge stdout: {error}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => return output,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("timed out draining bridge stdout")
                }
            }
        }
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn assert_attached(bridge: &BridgeProcess) {
    let first = bridge.recv_json();
    assert_eq!(first["jsonrpc"], "2.0", "{first}");
    assert!(
        first["result"].is_object(),
        "automatic attach response: {first}"
    );
}

#[test]
fn bridge_automatically_attaches_and_relays_requests_and_completion_events() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut bridge = BridgeProcess::spawn(process.pid, &["arf.execution_completed"]);

    // Send a numeric ID before reading the automatic attach response. The bridge must keep
    // that response's reserved string ID distinct from the client's request ID.
    bridge.send_line(r#"{"jsonrpc":"2.0","id":1,"method":"evaluate","params":{"code":"1 + 1"}}"#);
    let attach = bridge.recv_json();
    assert_eq!(attach["id"], "arf-ipc-attach", "{attach}");
    assert!(
        attach["result"].is_object(),
        "automatic attach response: {attach}"
    );

    // Keep stdin open while the response and its subscribed completion event arrive.
    let mut response = None;
    let mut completion = None;
    while response.is_none() || completion.is_none() {
        let message = bridge.recv_json();
        if message["id"] == 1 {
            response = Some(message);
        } else if message["params"]["topic"] == "arf.execution_completed" {
            completion = Some(message);
        }
    }
    let response = response.expect("evaluation response");
    let completion = completion.expect("execution completion event");
    assert_eq!(response["result"]["value"], "[1] 2", "{response}");
    assert_eq!(response["result"]["status"], "ok", "{response}");
    assert_eq!(
        completion["params"]["data"]["id"], response["result"]["execution_id"],
        "{completion}"
    );
    assert_eq!(completion["params"]["data"]["status"], "ok", "{completion}");
}

#[cfg(unix)]
fn fake_socket_server() -> (
    String,
    mpsc::Receiver<String>,
    thread::JoinHandle<()>,
    Option<tempfile::TempDir>,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::UnixListener;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("create fake IPC server runtime");
    let temp = tempfile::tempdir().expect("create fake IPC socket directory");
    let socket_path = temp.path().join("fake.sock");
    let listener = {
        let _guard = runtime.enter();
        UnixListener::bind(&socket_path).expect("bind fake IPC socket")
    };
    let (attach_tx, attach_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        runtime.block_on(async move {
            let (stream, _) = listener
                .accept()
                .await
                .expect("accept fake bridge connection");
            let mut line = String::new();
            BufReader::new(stream)
                .read_line(&mut line)
                .await
                .expect("read bridge attach request");
            let _ = attach_tx.send(line);
        });
    });
    (
        socket_path.to_string_lossy().into_owned(),
        attach_rx,
        server,
        Some(temp),
    )
}

#[cfg(windows)]
fn fake_socket_server() -> (
    String,
    mpsc::Receiver<String>,
    thread::JoinHandle<()>,
    Option<tempfile::TempDir>,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("create fake IPC server runtime");
    let pipe_name = format!(r"\\.\pipe\arf-bridge-test-{}", std::process::id());
    let pipe = {
        let _guard = runtime.enter();
        ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .expect("create fake IPC named pipe")
    };
    let (attach_tx, attach_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        runtime.block_on(async move {
            pipe.connect().await.expect("accept fake bridge connection");
            let mut line = String::new();
            BufReader::new(pipe)
                .read_line(&mut line)
                .await
                .expect("read bridge attach request");
            let _ = attach_tx.send(line);
        });
    });
    (pipe_name, attach_rx, server, None)
}

#[test]
fn bridge_reports_transport_error_when_server_closes_before_attach_reply() {
    let sessions_dir = tempfile::tempdir().expect("create temporary sessions directory");
    let (socket_path, attach_request, server, _socket_directory) = fake_socket_server();
    let session = format!(
        r#"{{"pid":{},"socket_path":{},"r_version":null,"r_home":null,"cwd":"/","started_at":"2026-01-01T00:00:00Z","session_type":"headless","log_file":null,"history_session_id":null}}"#,
        std::process::id(),
        serde_json::to_string(&socket_path).expect("encode socket path")
    );
    std::fs::write(
        sessions_dir
            .path()
            .join(format!("{}.json", std::process::id())),
        session,
    )
    .expect("write fake session metadata");

    let mut bridge = BridgeProcess::spawn_with_sessions_dir(
        std::process::id(),
        sessions_dir
            .path()
            .to_str()
            .expect("sessions path is UTF-8"),
    );
    let received = attach_request
        .recv_timeout(BRIDGE_TIMEOUT)
        .expect("fake server did not receive attach request");
    let attach: Value = serde_json::from_str(&received).expect("bridge sent JSON attach request");
    assert_eq!(attach["method"], "attach", "{attach}");

    let status = bridge.wait_for_exit(BRIDGE_TIMEOUT);
    let stderr = bridge.stderr_after_exit();
    assert_eq!(status.code(), Some(2), "stderr: {stderr}");
    let error: Value = serde_json::from_str(stderr.trim()).unwrap_or_else(|parse_error| {
        panic!("expected a JSON transport error on stderr ({parse_error}): {stderr:?}")
    });
    assert_eq!(error["error"]["code"], "TRANSPORT_ERROR", "{error}");
    assert!(
        bridge.stdin.is_some(),
        "bridge exited only after stdin was closed"
    );
    server.join().expect("fake IPC server thread");
}

#[test]
fn bridge_stdin_eof_ends_bridge_without_stopping_r() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut bridge = BridgeProcess::spawn(process.pid, &[]);
    assert_attached(&bridge);

    bridge.close_stdin();
    assert!(bridge.wait_for_exit(BRIDGE_TIMEOUT).success());

    let result = process
        .ipc_eval("40 + 2")
        .expect("R session should remain usable");
    assert!(result.success, "bridge EOF stopped R: {}", result.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout).expect("evaluation JSON")["value"],
        "[1] 42"
    );
}

#[test]
fn bridge_connection_disconnect_leaves_r_running() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut bridge = BridgeProcess::spawn(process.pid, &[]);
    assert_attached(&bridge);

    bridge.send_line(r#"this is not JSON"#);
    assert!(bridge.wait_for_exit(BRIDGE_TIMEOUT).success());
    assert!(
        bridge.stdin.is_some(),
        "bridge exited only after stdin was closed"
    );

    let result = process
        .ipc_eval("40 + 2")
        .expect("R session should remain usable");
    assert!(result.success, "disconnect stopped R: {}", result.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout).expect("evaluation JSON")["value"],
        "[1] 42"
    );
}

#[test]
fn bridge_exits_on_server_shutdown_while_stdin_remains_open() {
    let mut process = HeadlessProcess::spawn().expect("headless process should start");
    let mut bridge = BridgeProcess::spawn(process.pid, &[]);
    assert_attached(&bridge);

    bridge.send_line(r#"{"jsonrpc":"2.0","id":9,"method":"shutdown","params":{}}"#);
    assert!(bridge.wait_for_exit(BRIDGE_TIMEOUT).success());
    let shutdown = bridge.recv_json();
    assert_eq!(shutdown["id"], 9, "{shutdown}");
    assert_eq!(shutdown["result"]["accepted"], true, "{shutdown}");
    assert!(
        bridge.stdin.is_some(),
        "bridge exited only after stdin was closed"
    );
    process
        .wait_for_exit(BRIDGE_TIMEOUT)
        .expect("headless server should shut down");
}

#[test]
fn bridge_rejects_invalid_subscription_with_json_stderr_and_exit_code_four() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut bridge = BridgeProcess::spawn(process.pid, &[""]);
    let status = bridge.wait_for_exit(BRIDGE_TIMEOUT);
    let stderr = bridge.stderr_after_exit();
    assert_eq!(status.code(), Some(4), "stderr: {stderr}");
    let error: Value = serde_json::from_str(stderr.trim()).unwrap_or_else(|parse_error| {
        panic!("expected a JSON error on stderr ({parse_error}): {stderr:?}")
    });
    assert_eq!(error["error"]["code"], "INVALID_PARAMS", "{error}");
    assert!(bridge.stdout_after_exit().is_empty());
}

#[test]
fn bridge_exits_when_stdout_is_broken() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut bridge = BridgeProcess::spawn_with_broken_stdout(process.pid, &[]);
    let status = bridge.wait_for_exit(BRIDGE_TIMEOUT);
    let stderr = bridge.stderr_after_exit();
    assert_eq!(status.code(), Some(2), "stderr: {stderr}");
    let error: Value = serde_json::from_str(stderr.trim()).unwrap_or_else(|parse_error| {
        panic!("expected a JSON output error on stderr ({parse_error}): {stderr:?}")
    });
    assert_eq!(error["error"]["code"], "OUTPUT_ERROR", "{error}");
    let result = process
        .ipc_eval("6 * 7")
        .expect("R should survive bridge output failure");
    assert!(result.success, "{}", result.stderr);
}

#[test]
fn bridge_stdin_eof_exits_while_stdout_is_blocked() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut bridge = BridgeProcess::spawn_with_blocked_stdout(process.pid, &[]);
    let expression = "paste0('bridge-backpressure-', strrep('x', 262144L))";
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":8,"method":"evaluate","params":{{"code":"{expression}","visible":true}}}}"#
    );
    bridge.send_line(&request);

    // The completed history row proves R handled the request while stdout remains unread.
    wait_for_history_until(
        &process,
        &[],
        BRIDGE_TIMEOUT,
        Duration::from_millis(50),
        |history| {
            history["entries"].as_array().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry["command"] == expression && entry["exit_status"] == 0)
            })
        },
    );
    bridge.close_stdin();
    assert!(bridge.wait_for_exit(BRIDGE_TIMEOUT).success());

    let result = process
        .ipc_eval("6 * 7")
        .expect("R should remain usable after blocked stdout and EOF");
    assert!(result.success, "{}", result.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout).expect("evaluation JSON")["value"],
        "[1] 42"
    );
}

#[cfg(unix)]
#[test]
fn bridge_sigint_and_sigterm_exit_without_stopping_r() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    for signal in [
        nix::sys::signal::Signal::SIGINT,
        nix::sys::signal::Signal::SIGTERM,
    ] {
        let mut bridge = BridgeProcess::spawn(process.pid, &[]);
        assert_attached(&bridge);

        nix::sys::signal::kill(nix::unistd::Pid::from_raw(bridge.child.id() as i32), signal)
            .unwrap_or_else(|error| panic!("send {signal} to bridge: {error}"));
        assert!(
            bridge.wait_for_exit(BRIDGE_TIMEOUT).success(),
            "bridge did not exit cleanly after {signal}"
        );
    }

    let result = process
        .ipc_eval("21 * 2")
        .expect("R session should remain usable");
    assert!(result.success, "bridge signal stopped R: {}", result.stderr);
}
