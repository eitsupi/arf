//! Headless mode integration tests.
//!
//! These tests verify `arf headless` + `arf ipc` end-to-end without
//! requiring a terminal (PTY/ConPTY). This makes them runnable on
//! Windows CI where ConPTY cursor::position() is problematic.
//!
//! Each test spawns `arf headless`, waits for IPC readiness via session
//! file discovery, then uses `arf ipc eval` / `arf ipc send` CLI commands
//! to interact with R.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Timeout for waiting for IPC server to start.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Wrapper around a headless arf process.
///
/// Spawns `arf headless` and waits for IPC readiness by monitoring
/// stderr for the "IPC server listening on:" message.
struct HeadlessProcess {
    child: Child,
    pid: u32,
    _stderr_thread: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    stderr_output: Arc<Mutex<String>>,
}

impl HeadlessProcess {
    /// Spawn `arf headless` and wait for IPC server to be ready.
    fn spawn() -> Result<Self, String> {
        Self::spawn_with_args(&[])
    }

    /// Spawn `arf headless` with additional R flags and wait for IPC readiness.
    fn spawn_with_args(extra_args: &[&str]) -> Result<Self, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let mut cmd = Command::new(bin_path);
        cmd.arg("headless");
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn arf headless: {e}"))?;
        let pid = child.id();

        let stderr = child.stderr.take().expect("stderr should be piped");
        let stderr_output = Arc::new(Mutex::new(String::new()));
        let stderr_clone = Arc::clone(&stderr_output);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        // Channel to signal IPC readiness
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let mut ready_tx = Some(ready_tx);

        // Spawn a thread to read stderr and detect IPC readiness
        let stderr_thread = thread::spawn(move || {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                match line {
                    Ok(line) => {
                        if line.contains("IPC server listening on:") {
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                        if let Ok(mut output) = stderr_clone.lock() {
                            output.push_str(&line);
                            output.push('\n');
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Wait for IPC readiness
        match ready_rx.recv_timeout(STARTUP_TIMEOUT) {
            Ok(()) => {}
            Err(_) => {
                // Kill the process and report what we got
                let _ = child.kill();
                let output = stderr_output.lock().map(|s| s.clone()).unwrap_or_default();
                return Err(format!(
                    "Timeout waiting for headless IPC server to start. Stderr:\n{output}"
                ));
            }
        }

        Ok(HeadlessProcess {
            child,
            pid,
            _stderr_thread: Some(stderr_thread),
            shutdown,
            stderr_output,
        })
    }

    /// Run `arf ipc eval <code> --pid <pid>` and return (stdout, stderr, success).
    fn ipc_eval(&self, code: &str) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let output = Command::new(bin_path)
            .args(["ipc", "eval", code, "--pid", &self.pid.to_string()])
            .output()
            .map_err(|e| format!("Failed to run arf ipc eval: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        })
    }

    /// Run `arf ipc eval <code> --pid <pid> --visible` and return output.
    fn ipc_eval_visible(&self, code: &str) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let output = Command::new(bin_path)
            .args([
                "ipc",
                "eval",
                code,
                "--pid",
                &self.pid.to_string(),
                "--visible",
            ])
            .output()
            .map_err(|e| format!("Failed to run arf ipc eval --visible: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        })
    }

    /// Run `arf ipc send <code> --pid <pid>` and return output.
    fn ipc_send(&self, code: &str) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let output = Command::new(bin_path)
            .args(["ipc", "send", code, "--pid", &self.pid.to_string()])
            .output()
            .map_err(|e| format!("Failed to run arf ipc send: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        })
    }

    /// Run `arf ipc status --pid <pid>` and return output.
    fn ipc_status(&self) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let output = Command::new(bin_path)
            .args(["ipc", "status", "--pid", &self.pid.to_string()])
            .output()
            .map_err(|e| format!("Failed to run arf ipc status: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        })
    }

    /// Get the headless process's stderr output collected so far.
    #[allow(dead_code)]
    fn stderr_output(&self) -> String {
        self.stderr_output
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}

impl Drop for HeadlessProcess {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Output from an `arf ipc` CLI command.
#[derive(Debug)]
struct IpcOutput {
    stdout: String,
    stderr: String,
    success: bool,
}

// ===========================================================================
// Tests
// ===========================================================================

/// Test that `arf headless` starts and the IPC server becomes reachable.
#[test]
fn test_headless_starts_and_ipc_ready() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process.ipc_status().expect("ipc status should run");
    assert!(
        result.success,
        "ipc status should succeed. stdout: {}, stderr: {}",
        result.stdout, result.stderr
    );
    assert!(
        result
            .stdout
            .contains(&format!("PID:        {}", process.pid)),
        "status should show correct PID: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval` returns a visible R value.
#[test]
fn test_headless_eval_value() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(
        result.success,
        "eval should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("[1] 2"),
        "should capture R value: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval` captures stdout from `cat()`.
#[test]
fn test_headless_eval_stdout() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("cat('hello_headless\\n')")
        .expect("eval should run");
    assert!(
        result.success,
        "eval should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("hello_headless"),
        "should capture stdout: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval` reports R errors.
#[test]
fn test_headless_eval_error() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("stop('headless_error')")
        .expect("eval should run");
    // The CLI exits with non-zero on R errors
    assert!(
        !result.success,
        "eval should fail on R error. stdout: {}",
        result.stdout
    );
    assert!(
        result.stderr.contains("headless_error"),
        "should report error message: {}",
        result.stderr
    );
}

/// Test sequential evaluations: state persists across calls.
#[test]
fn test_headless_eval_sequential() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Assign a variable
    let r1 = process.ipc_eval("x <- 42").expect("first eval should run");
    assert!(r1.success, "first eval should succeed");

    // Use the variable
    let r2 = process.ipc_eval("x * 2").expect("second eval should run");
    assert!(r2.success, "second eval should succeed");
    assert!(
        r2.stdout.contains("[1] 84"),
        "should see variable from first eval: {}",
        r2.stdout
    );
}

/// Test that `arf ipc eval` captures both stdout and value.
#[test]
fn test_headless_eval_mixed_output() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("cat('before\\n'); 42")
        .expect("eval should run");
    assert!(result.success, "eval should succeed");
    assert!(
        result.stdout.contains("before"),
        "should capture stdout: {}",
        result.stdout
    );
    assert!(
        result.stdout.contains("[1] 42"),
        "should capture value: {}",
        result.stdout
    );
}

/// Test that `arf ipc send` (user_input) is accepted in headless mode.
#[test]
fn test_headless_user_input() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_send("invisible(NULL)")
        .expect("send should run");
    assert!(
        result.success,
        "send should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("Input accepted"),
        "should report acceptance: {}",
        result.stdout
    );
}

/// Test multiline R code evaluation in headless mode.
#[test]
fn test_headless_eval_multiline() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let code = r#"f <- function(x) x + 1; f(10)"#;
    let result = process.ipc_eval(code).expect("eval should run");
    assert!(result.success, "eval should succeed");
    assert!(
        result.stdout.contains("[1] 11"),
        "should evaluate multiline code: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval --visible` works in headless mode.
///
/// In headless mode there is no terminal, so visible vs silent is handled
/// identically (both use capture). This test verifies the `--visible` flag
/// is accepted and the output is still returned correctly.
#[test]
fn test_headless_eval_visible() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval_visible("cat('vis_test\\n'); 99")
        .expect("visible eval should run");
    assert!(
        result.success,
        "visible eval should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("vis_test"),
        "should capture stdout: {}",
        result.stdout
    );
    assert!(
        result.stdout.contains("[1] 99"),
        "should capture value: {}",
        result.stdout
    );
}

/// Test that `--vanilla` flag works in headless mode.
#[test]
fn test_headless_vanilla_flag() {
    let process =
        HeadlessProcess::spawn_with_args(&["--vanilla"]).expect("Failed to spawn with --vanilla");

    let result = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(
        result.success,
        "eval should succeed with --vanilla. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("[1] 2"),
        "should return result: {}",
        result.stdout
    );
}
