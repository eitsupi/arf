//! Headless mode integration tests.
//!
//! These tests verify `arf headless` + `arf ipc` end-to-end without
//! requiring a terminal (PTY/ConPTY). This makes them runnable on
//! Windows CI where ConPTY cursor::position() is problematic.
//!
//! Each test spawns `arf headless`, waits for IPC readiness by monitoring
//! stderr for the "IPC server listening on:" message, then uses
//! `arf ipc eval` / `arf ipc send` CLI commands to interact with R.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Timeout for waiting for IPC server to start.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Parse JSON from an IPC output, including stdout/stderr in the panic
/// message on failure for easier debugging.
fn parse_ipc_json(output: &IpcOutput) -> serde_json::Value {
    serde_json::from_str(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "failed to parse JSON: {e}\nstdout: {}\nstderr: {}",
            output.stdout, output.stderr
        )
    })
}

/// Run `arf ipc ...` directly and capture output.
fn run_ipc_command(args: &[&str]) -> std::process::Output {
    let bin_path = env!("CARGO_BIN_EXE_arf");
    Command::new(bin_path)
        .args(args)
        .output()
        .expect("run arf ipc")
}

/// Wrapper around a headless arf process.
///
/// Spawns `arf headless` and waits for IPC readiness by monitoring
/// stderr for the "IPC server listening on:" message.
struct HeadlessProcess {
    child: Child,
    pid: u32,
    _stderr_thread: Option<thread::JoinHandle<()>>,
    _stdout_thread: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    /// Collected stderr from the headless process (status messages, visible eval errors).
    stderr_output: Arc<Mutex<String>>,
    /// Collected stdout from the headless process (visible eval output).
    stdout_output: Arc<Mutex<String>>,
}

impl HeadlessProcess {
    /// Spawn `arf headless` and wait for IPC server to be ready.
    fn spawn() -> Result<Self, String> {
        Self::spawn_with_args(&[])
    }

    /// Spawn `arf headless` with additional R flags and wait for IPC readiness.
    fn spawn_with_args(extra_args: &[&str]) -> Result<Self, String> {
        Self::spawn_inner(&[], extra_args, None)
    }

    /// Spawn `arf headless` with global flags placed before the subcommand.
    fn spawn_with_pre_args(pre_args: &[&str]) -> Result<Self, String> {
        Self::spawn_inner(pre_args, &[], None)
    }

    /// Spawn with Windows creation flags (e.g., CREATE_NEW_PROCESS_GROUP).
    #[cfg(windows)]
    fn spawn_with_creation_flags(extra_args: &[&str], flags: u32) -> Result<Self, String> {
        Self::spawn_inner(&[], extra_args, Some(flags))
    }

    fn spawn_inner(
        pre_subcommand_args: &[&str],
        extra_args: &[&str],
        #[allow(unused)] creation_flags: Option<u32>,
    ) -> Result<Self, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");
        // When --quiet/--json is used, status messages are suppressed on stderr.
        // When --log-file is used, stderr is redirected to the file, so the
        // pipe is disconnected. In these cases, fall back to polling for readiness
        // instead of monitoring stderr for the "IPC server listening" message.
        let poll_for_readiness = extra_args.contains(&"--quiet")
            || extra_args.contains(&"--json")
            || extra_args.contains(&"--log-file");

        let mut cmd = Command::new(bin_path);
        for arg in pre_subcommand_args {
            cmd.arg(arg);
        }
        cmd.arg("headless");
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        #[cfg(windows)]
        if let Some(flags) = creation_flags {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(flags);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn arf headless: {e}"))?;
        let pid = child.id();

        let stderr = child.stderr.take().expect("stderr should be piped");
        let stdout = child.stdout.take().expect("stdout should be piped");
        let stderr_output = Arc::new(Mutex::new(String::new()));
        let stdout_output = Arc::new(Mutex::new(String::new()));
        let stderr_clone = Arc::clone(&stderr_output);
        let stdout_clone = Arc::clone(&stdout_output);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let shutdown_clone2 = Arc::clone(&shutdown);

        // Channel to signal IPC readiness (used when stderr is available)
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
                        if line.contains("IPC server listening on:")
                            && let Some(tx) = ready_tx.take()
                        {
                            let _ = tx.send(());
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

        // Spawn a thread to read stdout (visible eval output goes here)
        let stdout_thread = thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut buf = String::new();
            loop {
                if shutdown_clone2.load(Ordering::Relaxed) {
                    break;
                }
                buf.clear();
                match reader.read_line(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(mut output) = stdout_clone.lock() {
                            output.push_str(&buf);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Wait for IPC readiness
        if poll_for_readiness {
            // Stderr readiness message is not available (suppressed in --quiet
            // mode, or pipe disconnected in --log-file mode). Probe readiness
            // by running an actual RPC (`arf ipc eval "1"`) until it succeeds.
            // This ensures R is fully initialized and `set_r_at_prompt(true)`
            // has been called, unlike `ipc status` which only checks the
            // session file.
            let start = std::time::Instant::now();
            let mut last_probe_err = String::new();
            loop {
                if start.elapsed() > STARTUP_TIMEOUT {
                    let _ = child.kill();
                    let server_stderr = stderr_output.lock().map(|s| s.clone()).unwrap_or_default();
                    return Err(format!(
                        "Timeout waiting for IPC eval to succeed (polling mode).\n\
                         Server stderr:\n{server_stderr}\n\
                         Last probe error:\n{last_probe_err}"
                    ));
                }
                // Check if the process has exited early (e.g. error)
                if let Ok(Some(status)) = child.try_wait() {
                    let output = stderr_output.lock().map(|s| s.clone()).unwrap_or_default();
                    return Err(format!(
                        "Headless process exited early with {status}. Stderr:\n{output}"
                    ));
                }
                // Try a real RPC to confirm R is ready
                let probe = Command::new(bin_path)
                    .args([
                        "ipc",
                        "eval",
                        "1",
                        "--pid",
                        &pid.to_string(),
                        "--timeout",
                        "500",
                    ])
                    .output();
                match probe {
                    Ok(output) if output.status.success() => break,
                    Ok(output) => {
                        last_probe_err = String::from_utf8_lossy(&output.stderr).into_owned();
                    }
                    Err(e) => {
                        last_probe_err = e.to_string();
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        } else {
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
        }

        Ok(HeadlessProcess {
            child,
            pid,
            _stderr_thread: Some(stderr_thread),
            _stdout_thread: Some(stdout_thread),
            shutdown,
            stderr_output,
            stdout_output,
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
            exit_code: output.status.code(),
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
            exit_code: output.status.code(),
        })
    }

    /// Run `arf ipc eval <code> --pid <pid> --timeout <ms>` and return output.
    fn ipc_eval_with_timeout(&self, code: &str, timeout_ms: u64) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let output = Command::new(bin_path)
            .args([
                "ipc",
                "eval",
                code,
                "--pid",
                &self.pid.to_string(),
                "--timeout",
                &timeout_ms.to_string(),
            ])
            .output()
            .map_err(|e| format!("Failed to run arf ipc eval --timeout: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
            exit_code: output.status.code(),
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
            exit_code: output.status.code(),
        })
    }

    /// Run `arf ipc session --pid <pid>` and return output.
    fn ipc_session(&self) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let output = Command::new(bin_path)
            .args(["ipc", "session", "--pid", &self.pid.to_string()])
            .output()
            .map_err(|e| format!("Failed to run arf ipc session: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
            exit_code: output.status.code(),
        })
    }

    /// Run `arf ipc history --pid <pid>` with optional extra args and return output.
    fn ipc_history(&self, extra_args: &[&str]) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");
        let pid_str = self.pid.to_string();

        let mut args = vec!["ipc", "history", "--pid", &pid_str];
        args.extend_from_slice(extra_args);

        let output = Command::new(bin_path)
            .args(&args)
            .output()
            .map_err(|e| format!("Failed to run arf ipc history: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
            exit_code: output.status.code(),
        })
    }

    /// Run `arf ipc shutdown --pid <pid>` and return output.
    fn ipc_shutdown(&self) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let output = Command::new(bin_path)
            .args(["ipc", "shutdown", "--pid", &self.pid.to_string()])
            .output()
            .map_err(|e| format!("Failed to run arf ipc shutdown: {e}"))?;

        Ok(IpcOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
            exit_code: output.status.code(),
        })
    }

    /// Wait for the headless process to exit, with a timeout.
    /// Returns the `ExitStatus` on success.
    fn wait_for_exit(&mut self, timeout: Duration) -> Result<std::process::ExitStatus, String> {
        let start = std::time::Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => {
                    if start.elapsed() > timeout {
                        return Err("Process did not exit within timeout".to_string());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(format!("Error waiting for process: {e}")),
            }
        }
    }

    /// Get the headless process's stderr output collected so far.
    fn stderr_output(&self) -> String {
        self.stderr_output
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Get the headless process's stdout output collected so far.
    fn stdout_output(&self) -> String {
        self.stdout_output
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Get all output (stdout + stderr) from the headless process.
    fn server_output(&self) -> String {
        format!("{}{}", self.stdout_output(), self.stderr_output())
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
    exit_code: Option<i32>,
}

// ===========================================================================
// Tests
// ===========================================================================

/// Test that `arf headless` starts and the IPC server becomes reachable.
#[test]
fn test_headless_starts_and_ipc_ready() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process.ipc_session().expect("ipc session should run");
    assert!(
        result.success,
        "ipc session should succeed. stdout: {}, stderr: {}",
        result.stdout, result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["pid"].as_u64(),
        Some(process.pid as u64),
        "session should show correct PID: {}",
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

/// Test that `arf ipc eval` reports R errors in the JSON response.
#[test]
fn test_headless_eval_error() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("stop('headless_error')")
        .expect("eval should run");
    // R errors are returned as part of the JSON response (exit 0)
    assert!(
        result.success,
        "eval should succeed (R errors are in JSON, not exit code). stderr: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|s| s.contains("headless_error")),
        "should report error in JSON: {}",
        result.stdout
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
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["accepted"].as_bool(),
        Some(true),
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

/// Test that `arf ipc eval --visible` outputs to the headless process's stderr.
///
/// When `--visible` is used, the evaluated output should appear both in the
/// JSON-RPC response AND on the headless process's stdout/stderr (via
/// WriteConsoleEx passthrough). This is useful for monitoring/logging.
#[test]
fn test_headless_eval_visible() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Use a unique marker to avoid matching startup messages
    let result = process
        .ipc_eval_visible("cat('vis_marker_42\\n')")
        .expect("visible eval should run");
    assert!(
        result.success,
        "visible eval should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("vis_marker_42"),
        "JSON-RPC response should capture stdout: {}",
        result.stdout
    );

    // Give the headless process a moment to flush output
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Verify the output also appeared on the headless process's stdout/stderr.
    // R's cat() goes through WriteConsoleEx non-error channel → print! → stdout.
    let server_output = process.server_output();
    assert!(
        server_output.contains("vis_marker_42"),
        "visible eval output should appear on headless process output: {}",
        server_output
    );
}

/// Test that silent eval does NOT output to the headless process.
#[test]
fn test_headless_eval_silent_no_server_output() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("cat('silent_marker_99\\n')")
        .expect("eval should run");
    assert!(result.success, "eval should succeed");
    assert!(
        result.stdout.contains("silent_marker_99"),
        "JSON-RPC response should capture stdout: {}",
        result.stdout
    );

    // Give a moment for any output to flush
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Silent eval should NOT appear on the headless process
    let server_output = process.server_output();
    assert!(
        !server_output.contains("silent_marker_99"),
        "silent eval output should NOT appear on headless process output: {}",
        server_output
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

/// Test that --timeout option works: a fast eval completes within timeout.
#[test]
fn test_headless_eval_timeout_sufficient() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Fast expression with generous timeout should succeed
    let result = process
        .ipc_eval_with_timeout("1 + 1", 30000)
        .expect("eval with timeout should run");
    assert!(result.success, "should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("[1] 2"),
        "should return result: {}",
        result.stdout
    );
}

/// Test that --timeout option works: a slow eval times out.
#[test]
fn test_headless_eval_timeout_exceeded() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Sys.sleep(10) with a 1-second timeout should fail
    let result = process
        .ipc_eval_with_timeout("Sys.sleep(10)", 1000)
        .expect("eval with timeout should run");
    assert!(
        !result.success,
        "should fail due to timeout. stdout: {}, stderr: {}",
        result.stdout, result.stderr
    );
    assert!(
        result.stderr.contains("timed out"),
        "should mention timeout: {}",
        result.stderr
    );
}

/// Test that `arf ipc shutdown` gracefully stops a headless process.
#[test]
fn test_headless_shutdown_via_ipc() {
    let mut process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Verify it's running
    let session = process.ipc_session().expect("session should work");
    assert!(session.success, "should be running");

    // Send shutdown
    let result = process.ipc_shutdown().expect("shutdown should run");
    assert!(
        result.success,
        "shutdown should succeed. stderr: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["accepted"].as_bool(),
        Some(true),
        "should report acceptance: {}",
        result.stdout
    );

    // Process should exit within a few seconds
    process
        .wait_for_exit(Duration::from_secs(10))
        .expect("headless process should exit after shutdown");
}

/// Test that help pages are captured via the custom pager instead of
/// spawning an interactive pager like `less`.
#[test]
fn test_headless_help_does_not_hang() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // ?mean triggers R's help system which would normally open a pager.
    // With our custom pager, the help text should be captured in stdout.
    let result = process
        .ipc_eval_with_timeout("?mean", 15000)
        .expect("help eval should run");
    assert!(
        result.success,
        "help should succeed without hanging. stderr: {}",
        result.stderr
    );
    // The help text for `mean` should contain the word "mean" somewhere
    assert!(
        result.stdout.to_lowercase().contains("mean"),
        "help output should contain 'mean': {}",
        result.stdout
    );
}

/// Test that plot() does not hang or error in headless mode.
/// The graphics device should default to a file-based device (png/pdf).
#[test]
fn test_headless_plot_does_not_hang() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // plot() would normally try to open X11/quartz. In headless mode,
    // our custom device function should create a file-based device instead.
    let result = process
        .ipc_eval_with_timeout(
            "plot(1:10); dev_name <- names(dev.cur()); dev.off(); cat(dev_name)",
            15000,
        )
        .expect("plot eval should run");
    assert!(
        result.success,
        "plot should succeed without hanging. stderr: {}",
        result.stderr
    );
    // Verify the device is non-interactive: png/pdf from our custom device,
    // or quartz_off_screen on macOS (quartz works headlessly unlike X11)
    let stdout = &result.stdout;
    assert!(
        stdout.contains("png") || stdout.contains("pdf") || stdout.contains("quartz_off_screen"),
        "graphics device should be non-interactive, got: {}",
        stdout
    );
}

/// Test that browseURL() prints the URL to stdout instead of opening a browser.
#[test]
fn test_headless_browse_url_does_not_hang() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval_with_timeout("browseURL('https://example.com')", 15000)
        .expect("browseURL eval should run");
    assert!(
        result.success,
        "browseURL should succeed without hanging. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("https://example.com"),
        "URL should be captured in stdout: {}",
        result.stdout
    );
}

/// Test that --bind allows specifying a custom socket path.
#[cfg(unix)]
#[test]
fn test_headless_bind_custom_socket() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let sock_path = tmp.path().join("custom.sock");
    let sock_str = sock_path.display().to_string();

    let process = HeadlessProcess::spawn_with_args(&["--bind", &sock_str])
        .expect("Failed to spawn headless with --bind");

    // The custom socket file should exist
    assert!(
        sock_path.exists(),
        "custom socket file should exist at: {}",
        sock_str
    );

    // IPC should work via the session discovery (which picks up the custom path)
    let result = process.ipc_eval("1 + 1").expect("eval should work");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("[1] 2"),
        "should return result: {}",
        result.stdout
    );

    // stderr should mention the custom path
    let stderr = process.stderr_output();
    assert!(
        stderr.contains(&sock_str),
        "stderr should mention custom socket path: {}",
        stderr
    );
}

/// Test that --pid-file writes the PID and is cleaned up on shutdown.
#[test]
fn test_headless_pid_file() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let pid_path = tmp.path().join("arf.pid");
    let pid_str = pid_path.display().to_string();

    let mut process = HeadlessProcess::spawn_with_args(&["--pid-file", &pid_str])
        .expect("Failed to spawn headless with --pid-file");

    // PID file is written right after the IPC server starts. Poll until the
    // file exists AND has non-empty content to avoid reading between create
    // and write.
    let start = std::time::Instant::now();
    let pid_content = loop {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "PID file should appear with content at: {}",
            pid_str
        );
        if let Ok(content) = std::fs::read_to_string(&pid_path)
            && !content.is_empty()
        {
            break content;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let expected_pid = process.pid.to_string();
    assert_eq!(
        pid_content.trim(),
        expected_pid,
        "PID file should contain process PID"
    );

    // Shutdown via IPC and verify PID file is cleaned up
    let result = process.ipc_shutdown().expect("shutdown should run");
    assert!(result.success, "shutdown should succeed");

    process
        .wait_for_exit(Duration::from_secs(10))
        .expect("headless process should exit after shutdown");

    // PID file should be removed on clean shutdown
    assert!(
        !pid_path.exists(),
        "PID file should be removed after shutdown"
    );
}

/// Test that --quiet suppresses status messages on stderr.
#[test]
fn test_headless_quiet_mode() {
    let process = HeadlessProcess::spawn_with_args(&["--quiet"])
        .expect("Failed to spawn headless with --quiet");

    // IPC should still work
    let result = process.ipc_eval("1 + 1").expect("eval should work");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("[1] 2"),
        "should return result: {}",
        result.stdout
    );

    // stderr should NOT contain the usual status messages
    let stderr = process.stderr_output();
    assert!(
        !stderr.contains("IPC server listening on:"),
        "quiet mode should suppress IPC listening message, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("Headless mode ready"),
        "quiet mode should suppress ready message, got: {}",
        stderr
    );
}

/// Test that --log-file redirects log output to a file.
#[test]
fn test_headless_log_file() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let log_path = tmp.path().join("arf.log");
    let log_str = log_path.display().to_string();

    let process = HeadlessProcess::spawn_with_args(&["--log-file", &log_str])
        .expect("Failed to spawn headless with --log-file");

    // Run a simple eval to ensure the server is working
    let result = process.ipc_eval("1 + 1").expect("eval should work");
    assert!(result.success, "eval should succeed: {}", result.stderr);

    // The log file should exist (env_logger writes to it)
    assert!(log_path.exists(), "log file should exist at: {}", log_str);

    let log_content = std::fs::read_to_string(&log_path).unwrap_or_default();

    // In headless mode, stderr is redirected to the log file via dup2.
    // Status messages (eprintln) should now appear in the log file.
    assert!(
        log_content.contains("Headless mode ready"),
        "log file should contain status messages (stderr is redirected): {}",
        log_content
    );

    // stderr pipe should be empty (disconnected by dup2 redirect)
    let stderr = process.stderr_output();
    assert!(
        stderr.trim().is_empty(),
        "stderr pipe should be empty when --log-file redirects stderr, but got: {}",
        stderr
    );
}

/// Helper: test that a Unix signal triggers graceful shutdown with PID file cleanup.
#[cfg(unix)]
fn assert_signal_graceful_shutdown(signal: nix::sys::signal::Signal) {
    use nix::sys::signal;
    use nix::unistd::Pid;

    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let pid_path = tmp.path().join("arf.pid");
    let pid_str = pid_path.display().to_string();

    let mut process = HeadlessProcess::spawn_with_args(&["--pid-file", &pid_str])
        .expect("Failed to spawn headless with --pid-file");

    // Wait for "Headless mode ready" on stderr, which is printed after the
    // signal handler has been installed. This avoids a race where the signal
    // arrives before the handler is set up.
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(10) {
            panic!(
                "Headless mode should become ready.\nServer output:\n{}",
                process.server_output()
            );
        }
        // Fail fast if the process has already exited
        if let Ok(Some(status)) = process.child.try_wait() {
            panic!(
                "Headless process exited early with {status}.\nServer output:\n{}",
                process.server_output()
            );
        }
        if process.stderr_output().contains("Headless mode ready") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // PID file should also exist by now (written before the handler)
    assert!(pid_path.exists(), "PID file should exist at: {}", pid_str);

    // Send the signal
    signal::kill(Pid::from_raw(process.pid as i32), signal)
        .unwrap_or_else(|e| panic!("failed to send {signal}: {e}"));

    // Process should exit gracefully
    let status = process
        .wait_for_exit(Duration::from_secs(10))
        .unwrap_or_else(|e| panic!("headless process should exit after {signal}: {e}"));
    assert!(
        status.success(),
        "headless process should exit cleanly after {signal}, got: {status}"
    );

    // PID file should be cleaned up
    assert!(
        !pid_path.exists(),
        "PID file should be removed after {signal} shutdown"
    );
}

/// Test that SIGTERM triggers graceful shutdown with PID file cleanup.
#[cfg(unix)]
#[test]
fn test_headless_sigterm_shutdown() {
    assert_signal_graceful_shutdown(nix::sys::signal::Signal::SIGTERM);
}

/// Test that SIGHUP triggers graceful shutdown with PID file cleanup.
#[cfg(unix)]
#[test]
fn test_headless_sighup_shutdown() {
    assert_signal_graceful_shutdown(nix::sys::signal::Signal::SIGHUP);
}

/// Test that Ctrl+C triggers graceful shutdown with PID file cleanup.
///
/// On Unix, sends SIGINT directly. On Windows, uses CTRL_BREAK_EVENT via
/// CREATE_NEW_PROCESS_GROUP + GenerateConsoleCtrlEvent, which is the only
/// way to signal a specific child process (CTRL_C_EVENT cannot target a
/// single process). The ctrlc crate handles both equivalently.
#[cfg(unix)]
#[test]
fn test_headless_ctrlc_shutdown() {
    assert_signal_graceful_shutdown(nix::sys::signal::Signal::SIGINT);
}

/// See [`test_headless_ctrlc_shutdown`] for rationale.
#[cfg(windows)]
#[test]
fn test_headless_ctrlc_shutdown() {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let pid_path = tmp.path().join("arf.pid");
    let pid_str = pid_path.display().to_string();

    let mut process = HeadlessProcess::spawn_with_creation_flags(
        &["--pid-file", &pid_str],
        CREATE_NEW_PROCESS_GROUP,
    )
    .expect("Failed to spawn headless with --pid-file");

    // Wait for "Headless mode ready" on stderr (signal handler is installed by then)
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(10) {
            panic!(
                "Headless mode should become ready.\nServer output:\n{}",
                process.server_output()
            );
        }
        if let Ok(Some(status)) = process.child.try_wait() {
            panic!(
                "Headless process exited early with {status}.\nServer output:\n{}",
                process.server_output()
            );
        }
        if process.stderr_output().contains("Headless mode ready") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(pid_path.exists(), "PID file should exist");

    // Send CTRL_BREAK_EVENT to the child's process group
    let result = unsafe {
        windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent(
            windows_sys::Win32::System::Console::CTRL_BREAK_EVENT,
            process.pid,
        )
    };
    assert!(
        result != 0,
        "GenerateConsoleCtrlEvent failed: {}",
        std::io::Error::last_os_error()
    );

    // Process should exit within timeout (not hang)
    let status = process
        .wait_for_exit(Duration::from_secs(10))
        .unwrap_or_else(|e| {
            panic!(
                "headless process should exit after CTRL_BREAK: {e}\nServer output:\n{}",
                process.server_output()
            )
        });

    assert!(
        status.success(),
        "headless process should exit cleanly after CTRL_BREAK, got: {status}\n\
         Server output:\n{}",
        process.server_output()
    );

    // PID file should be cleaned up
    assert!(
        !pid_path.exists(),
        "PID file should be removed after CTRL_BREAK shutdown\nServer output:\n{}",
        process.server_output()
    );
}

/// Test that headless mode persists evaluated commands to the history database.
///
/// Verifies that:
/// - IPC evaluate commands are saved with correct command_line and exit_status
/// - Metadata fields (hostname, cwd) are populated
/// - Errors are recorded with exit_status=1
/// - The --history-dir flag controls the database location
#[test]
fn test_headless_history_persistence() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process = HeadlessProcess::spawn_with_args(&["--history-dir", history_dir])
        .expect("Failed to spawn headless with --history-dir");

    // Run a successful command
    let r1 = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(r1.success, "first eval should succeed");

    // Run a command that errors (R errors are returned in JSON, exit 0)
    let r2 = process
        .ipc_eval("stop('test_error')")
        .expect("error eval should run");
    assert!(r2.success, "eval should succeed (R error is in JSON)");

    // Run a send (user_input) command
    let r3 = process
        .ipc_send("invisible(NULL)")
        .expect("send should run");
    assert!(r3.success, "send should succeed");

    // Whitespace-only commands should NOT be persisted to history.
    let _ = process
        .ipc_eval("   \n")
        .expect("whitespace eval should run");
    let _ = process
        .ipc_send("  \t  ")
        .expect("whitespace send should run");

    // Small delay to let SQLite flush
    std::thread::sleep(Duration::from_millis(200));

    // Read the history database directly
    let db_path = tmp.path().join("r.db");
    assert!(db_path.exists(), "history database should exist");

    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open history db");

    let mut stmt = conn
        .prepare(
            "SELECT command_line, exit_status, hostname, cwd \
             FROM history ORDER BY id",
        )
        .expect("prepare query");
    let rows: Vec<_> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect rows");

    // Filter to just our known commands for assertion stability.
    let success_row = rows.iter().find(|r| r.0 == "1 + 1");
    let error_row = rows.iter().find(|r| r.0 == "stop('test_error')");
    let send_row = rows.iter().find(|r| r.0 == "invisible(NULL)");

    // Successful eval
    let success_row = success_row.expect("should find '1 + 1' in history");
    assert_eq!(
        success_row.1,
        Some(0),
        "successful eval should have exit_status=0"
    );
    assert!(success_row.2.is_some(), "hostname should be populated");
    assert!(success_row.3.is_some(), "cwd should be populated");

    // Error eval
    let error_row = error_row.expect("should find error command in history");
    assert_eq!(error_row.1, Some(1), "error eval should have exit_status=1");

    // user_input (send)
    let send_row = send_row.expect("should find send command in history");
    assert_eq!(send_row.1, Some(0), "send should have exit_status=0");

    // Whitespace-only commands should not appear in history.
    let whitespace_rows: Vec<_> = rows.iter().filter(|r| r.0.trim().is_empty()).collect();
    assert!(
        whitespace_rows.is_empty(),
        "whitespace-only commands should not be persisted to history"
    );
}

/// Test that --no-history prevents history from being saved in headless mode.
#[test]
fn test_headless_no_history_flag() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process = HeadlessProcess::spawn_with_args(&["--history-dir", history_dir, "--no-history"])
        .expect("Failed to spawn headless with --no-history");

    // Run a command
    let result = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(result.success, "eval should succeed");

    // History database should NOT be created
    let db_path = tmp.path().join("r.db");
    assert!(
        !db_path.exists(),
        "history database should not exist with --no-history"
    );
}

/// Test that --json outputs valid JSON with session info to stdout.
#[test]
fn test_headless_json_output() {
    let process =
        HeadlessProcess::spawn_with_args(&["--json"]).expect("Failed to spawn with --json");

    // Wait for the stdout reader thread to capture the JSON output.
    // spawn_with_args already confirmed IPC readiness, so the JSON has been
    // written; we just need the reader thread to catch up.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while process.stdout_output().trim().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for JSON on stdout"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let stdout = process.stdout_output();

    // stdout should contain valid JSON with expected fields
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nstdout: {stdout}"));

    assert_eq!(
        json["pid"].as_u64().unwrap() as u32,
        process.pid,
        "JSON pid should match process PID"
    );
    assert!(
        json["socket_path"].is_string(),
        "JSON should have socket_path: {json}"
    );
    assert!(
        json["r_version"].is_string() || json["r_version"].is_null(),
        "JSON r_version should be a string or null: {json}"
    );
    assert!(json["cwd"].is_string(), "JSON should have cwd: {json}");
    assert!(
        json["started_at"].is_string(),
        "JSON should have started_at: {json}"
    );
    assert!(
        json["log_file"].is_null(),
        "JSON log_file should be null without --log-file: {json}"
    );
    assert!(
        json["warnings"].is_array(),
        "JSON should have warnings array: {json}"
    );

    // IPC should still work
    let result = process.ipc_eval("1 + 1").expect("eval should work");
    assert!(result.success, "eval should succeed: {}", result.stderr);

    // stderr should NOT contain status messages (--json implies --quiet)
    let stderr = process.stderr_output();
    assert!(
        !stderr.contains("IPC server listening on:"),
        "json mode should suppress IPC listening message, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("Headless mode ready"),
        "json mode should suppress ready message, got: {}",
        stderr
    );
}

// ── IPC history query tests ─────────────────────────────────────────────

/// Test basic `arf ipc history` query returns evaluated commands.
#[test]
fn test_ipc_history_basic() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    // Evaluate a few commands
    let r1 = process.ipc_eval("1 + 1").expect("eval 1");
    assert!(r1.success);
    let r2 = process.ipc_eval("cat('hello')").expect("eval 2");
    assert!(r2.success);

    // Small delay for SQLite flush
    std::thread::sleep(Duration::from_millis(200));

    // Query history
    let result = process.ipc_history(&[]).expect("history query");
    assert!(result.success, "history should succeed: {}", result.stderr);

    let json = parse_ipc_json(&result);
    let entries = json["entries"].as_array().expect("entries should be array");

    // Should contain both commands (newest first)
    assert!(
        entries.len() >= 2,
        "should have at least 2 entries, got {}: {json}",
        entries.len()
    );

    let commands: Vec<&str> = entries
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();
    assert!(
        commands.contains(&"1 + 1"),
        "should contain '1 + 1': {commands:?}"
    );
    assert!(
        commands.contains(&"cat('hello')"),
        "should contain cat('hello'): {commands:?}"
    );

    // Should have a session_id
    assert!(
        json["session_id"].is_number(),
        "should have session_id: {json}"
    );
}

/// Test `--limit` flag restricts the number of returned entries.
#[test]
fn test_ipc_history_limit() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    // Evaluate 3 commands
    for i in 1..=3 {
        let r = process
            .ipc_eval(&format!("{i} + {i}"))
            .expect("eval should run");
        assert!(r.success);
    }

    std::thread::sleep(Duration::from_millis(200));

    // Query with limit=2
    let result = process
        .ipc_history(&["--limit", "2"])
        .expect("history query");
    assert!(result.success);

    let json = parse_ipc_json(&result);
    let entries = json["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2, "should return exactly 2 entries: {json}");
}

/// Test `--grep` flag filters by command substring.
#[test]
fn test_ipc_history_grep() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    let r1 = process.ipc_eval("print('apple')").expect("eval 1");
    assert!(r1.success);
    let r2 = process.ipc_eval("cat('banana')").expect("eval 2");
    assert!(r2.success);
    let r3 = process.ipc_eval("print('apricot')").expect("eval 3");
    assert!(r3.success);

    std::thread::sleep(Duration::from_millis(200));

    // Search for "apple"
    let result = process
        .ipc_history(&["--grep", "apple"])
        .expect("history grep");
    assert!(result.success);

    let json = parse_ipc_json(&result);
    let entries = json["entries"].as_array().expect("entries array");

    let commands: Vec<&str> = entries
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();
    assert!(
        commands.iter().all(|c| c.contains("apple")),
        "all results should contain 'apple': {commands:?}"
    );
    assert!(
        !commands.iter().any(|c| c.contains("banana")),
        "should not contain 'banana': {commands:?}"
    );
}

/// Test that the default query returns only the current session's entries.
///
/// By default (without `--all-sessions`), history is scoped to the
/// current session. All returned entries must share the session_id.
#[test]
fn test_ipc_history_default_session_scoped() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    let r1 = process.ipc_eval("42").expect("eval");
    assert!(r1.success);

    std::thread::sleep(Duration::from_millis(200));

    // Default query — should be scoped to the current session
    let result = process.ipc_history(&[]).expect("history default");
    assert!(result.success);

    let json = parse_ipc_json(&result);
    let entries = json["entries"].as_array().expect("entries array");
    assert!(
        !entries.is_empty(),
        "default query should find entries: {json}"
    );

    // All entries should have the same session_id as the response
    let session_id = json["session_id"].as_i64().expect("session_id");
    for entry in entries {
        assert_eq!(
            entry["session_id"].as_i64(),
            Some(session_id),
            "all entries should match session_id: {entry}"
        );
    }
}

/// Test that history entries include metadata (timestamp, cwd, exit_status).
#[test]
fn test_ipc_history_metadata() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    let r1 = process.ipc_eval("1 + 1").expect("eval success");
    assert!(r1.success);
    let r2 = process.ipc_eval("stop('oops')").expect("eval error");
    assert!(r2.success, "eval should succeed (R error is in JSON)");

    std::thread::sleep(Duration::from_millis(200));

    let result = process.ipc_history(&[]).expect("history query");
    assert!(result.success);

    let json = parse_ipc_json(&result);
    let entries = json["entries"].as_array().expect("entries array");

    let success_entry = entries
        .iter()
        .find(|e| e["command"].as_str() == Some("1 + 1"))
        .expect("should find success entry");
    assert!(
        success_entry["timestamp"].is_string(),
        "should have timestamp: {success_entry}"
    );
    assert!(
        success_entry["cwd"].is_string(),
        "should have cwd: {success_entry}"
    );
    assert_eq!(
        success_entry["exit_status"].as_i64(),
        Some(0),
        "success should have exit_status=0: {success_entry}"
    );

    let error_entry = entries
        .iter()
        .find(|e| e["command"].as_str() == Some("stop('oops')"))
        .expect("should find error entry");
    assert_eq!(
        error_entry["exit_status"].as_i64(),
        Some(1),
        "error should have exit_status=1: {error_entry}"
    );
}

/// Test `--since` filters history entries by timestamp.
#[test]
fn test_ipc_history_since_filter() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();
    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    let r = process.ipc_eval("1 + 1").expect("eval");
    assert!(r.success);
    std::thread::sleep(Duration::from_millis(200));

    let future = process
        .ipc_history(&["--since", "2999-01-01"])
        .expect("history since future");
    assert!(future.success, "history should succeed: {}", future.stderr);
    let future_json = parse_ipc_json(&future);
    let future_entries = future_json["entries"].as_array().expect("entries array");
    assert!(
        future_entries.is_empty(),
        "future since filter should return no entries: {future_json}"
    );

    let past = process
        .ipc_history(&["--since", "1970-01-01"])
        .expect("history since past");
    assert!(past.success, "history should succeed: {}", past.stderr);
    let past_json = parse_ipc_json(&past);
    let past_entries = past_json["entries"].as_array().expect("entries array");
    assert!(
        past_entries
            .iter()
            .any(|e| e["command"].as_str() == Some("1 + 1")),
        "past since filter should include command: {past_json}"
    );
}

/// Test `--cwd` exact-match filtering.
#[test]
fn test_ipc_history_cwd_filter() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();
    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    let r = process.ipc_eval("1 + 1").expect("eval");
    assert!(r.success);
    std::thread::sleep(Duration::from_millis(200));

    let cwd = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let match_result = process
        .ipc_history(&["--cwd", &cwd])
        .expect("history cwd match");
    assert!(match_result.success, "history should succeed");
    let match_json = parse_ipc_json(&match_result);
    let match_entries = match_json["entries"].as_array().expect("entries array");
    assert!(
        !match_entries.is_empty(),
        "cwd match should return entries: {match_json}"
    );

    let miss_result = process
        .ipc_history(&["--cwd", "/definitely/nonexistent/cwd"])
        .expect("history cwd miss");
    assert!(miss_result.success, "history should succeed");
    let miss_json = parse_ipc_json(&miss_result);
    let miss_entries = miss_json["entries"].as_array().expect("entries array");
    assert!(
        miss_entries.is_empty(),
        "cwd miss should return no entries: {miss_json}"
    );
}

/// Test `--all-sessions` includes entries from another running headless session.
#[test]
fn test_ipc_history_all_sessions() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let p1 = HeadlessProcess::spawn_with_args(&["--history-dir", history_dir])
        .expect("spawn headless #1");
    let p2 = HeadlessProcess::spawn_with_args(&["--history-dir", history_dir])
        .expect("spawn headless #2");

    let r1 = p1.ipc_eval("cmd_from_p1 <- 1").expect("eval p1");
    assert!(r1.success);
    let r2 = p2.ipc_eval("cmd_from_p2 <- 2").expect("eval p2");
    assert!(r2.success);
    std::thread::sleep(Duration::from_millis(250));

    let default_result = p1.ipc_history(&[]).expect("history default");
    assert!(default_result.success, "history default should succeed");
    let default_json = parse_ipc_json(&default_result);
    let default_commands: Vec<&str> = default_json["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();
    assert!(
        default_commands.contains(&"cmd_from_p1 <- 1"),
        "default history should include own command: {default_commands:?}"
    );
    assert!(
        !default_commands.contains(&"cmd_from_p2 <- 2"),
        "default history should not include other session command: {default_commands:?}"
    );

    let all_result = p1
        .ipc_history(&["--all-sessions"])
        .expect("history all-sessions");
    assert!(all_result.success, "history all-sessions should succeed");
    let all_json = parse_ipc_json(&all_result);
    let all_commands: Vec<&str> = all_json["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();
    assert!(
        all_commands.contains(&"cmd_from_p1 <- 1"),
        "all-sessions should include own command: {all_commands:?}"
    );
    assert!(
        all_commands.contains(&"cmd_from_p2 <- 2"),
        "all-sessions should include other session command: {all_commands:?}"
    );
}

/// Test that history returns an error when history is disabled.
#[test]
fn test_ipc_history_disabled() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process = HeadlessProcess::spawn_with_args(&["--history-dir", history_dir, "--no-history"])
        .expect("spawn headless");

    let result = process.ipc_history(&[]).expect("history query");
    // Should fail because history is not configured
    assert!(
        !result.success,
        "history should fail when disabled: stdout={}, stderr={}",
        result.stdout, result.stderr
    );
}

// ── Exit code and structured error tests ─────────────────────────────

/// Test that `arf ipc eval --pid <wrong>` exits with code 3 (SESSION_NOT_FOUND)
/// and produces structured JSON error on stderr.
#[test]
fn test_ipc_exit_code_session_not_found() {
    let bin_path = env!("CARGO_BIN_EXE_arf");

    // Derive a PID unlikely to match any running arf session.
    let fake_pid = std::process::id().saturating_add(900_000).to_string();

    let output = Command::new(bin_path)
        .args(["ipc", "eval", "1", "--pid", &fake_pid])
        .output()
        .expect("should run");

    assert_eq!(
        output.status.code(),
        Some(3),
        "exit code should be 3 (session)"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("stderr should be JSON: {e}\nstderr: {stderr}"));
    assert_eq!(json["error"]["code"].as_str(), Some("SESSION_NOT_FOUND"));
    assert!(json["error"]["message"].as_str().is_some());
    assert!(json["error"]["hint"].as_str().is_some());
}

/// Test that omitting `--pid` with multiple sessions returns
/// `SESSION_AMBIGUOUS` (exit code 3).
#[test]
fn test_ipc_exit_code_session_ambiguous() {
    let p1 = HeadlessProcess::spawn().expect("spawn headless #1");
    let p2 = HeadlessProcess::spawn().expect("spawn headless #2");
    let _keep_alive = (&p1, &p2);

    let output = run_ipc_command(&["ipc", "eval", "1"]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "exit code should be 3 (session ambiguous/not found)"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("stderr should be JSON: {e}\nstderr: {stderr}"));
    assert_eq!(json["error"]["code"].as_str(), Some("SESSION_AMBIGUOUS"));
    assert!(json["error"]["message"].as_str().is_some());
    assert!(json["error"]["hint"].as_str().is_some());
}

/// Test that `arf ipc list` outputs valid JSON even with no sessions.
#[test]
fn test_ipc_list_empty_json() {
    // This test runs without a headless process, so list should return
    // an empty sessions array (or whatever sessions are running).
    let bin_path = env!("CARGO_BIN_EXE_arf");
    let output = Command::new(bin_path)
        .args(["ipc", "list"])
        .output()
        .expect("should run");

    assert!(output.status.success(), "list should always succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout should be JSON: {e}\nstdout: {stdout}"));
    assert!(json["sessions"].is_array(), "should have sessions array");
}

/// Test that transport failures produce `TRANSPORT_ERROR` (exit code 2).
///
/// Simulates transport failure by rewriting a live session file to reference
/// a nonexistent socket path, then invoking `arf ipc eval --pid <pid>`.
#[test]
fn test_ipc_exit_code_transport_error() {
    struct SessionFileRestoreGuard {
        path: std::path::PathBuf,
        original: String,
    }

    impl Drop for SessionFileRestoreGuard {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.path, &self.original);
        }
    }

    let process = HeadlessProcess::spawn().expect("spawn headless");

    let session_path = dirs::cache_dir()
        .expect("cache dir")
        .join("arf")
        .join("sessions")
        .join(format!("{}.json", process.pid));
    let session_raw = std::fs::read_to_string(&session_path)
        .unwrap_or_else(|e| panic!("read session file {}: {e}", session_path.display()));
    let _restore_guard = SessionFileRestoreGuard {
        path: session_path.clone(),
        original: session_raw.clone(),
    };
    let mut session_json: serde_json::Value = serde_json::from_str(&session_raw)
        .unwrap_or_else(|e| panic!("parse session file {}: {e}", session_path.display()));

    #[cfg(unix)]
    let bogus_socket = format!("/tmp/arf-missing-{}.sock", process.pid);
    #[cfg(windows)]
    let bogus_socket = format!(r"\\.\pipe\arf-missing-{}", process.pid);

    session_json["socket_path"] = serde_json::Value::String(bogus_socket);
    std::fs::write(
        &session_path,
        serde_json::to_string_pretty(&session_json).expect("serialize session file"),
    )
    .unwrap_or_else(|e| panic!("rewrite session file {}: {e}", session_path.display()));

    let pid_arg = process.pid.to_string();
    let output = run_ipc_command(&["ipc", "eval", "1", "--pid", &pid_arg]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code should be 2 (transport)"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("stderr should be JSON: {e}\nstderr: {stderr}"));
    assert_eq!(json["error"]["code"].as_str(), Some("TRANSPORT_ERROR"));
    assert!(json["error"]["message"].as_str().is_some());
}

/// Test that protocol errors (e.g. timeout) produce exit code 4 and
/// structured JSON error on stderr.
#[test]
fn test_ipc_exit_code_protocol_error() {
    let process = HeadlessProcess::spawn().expect("spawn headless");

    // Use a very short timeout to trigger a protocol-level timeout error
    let result = process
        .ipc_eval_with_timeout("Sys.sleep(10)", 500)
        .expect("eval should run");

    assert!(!result.success, "should fail due to timeout");
    assert_eq!(
        result.exit_code,
        Some(4),
        "exit code should be 4 (protocol)"
    );

    let json: serde_json::Value = serde_json::from_str(&result.stderr)
        .unwrap_or_else(|e| panic!("stderr should be JSON: {e}\nstderr: {}", result.stderr));
    assert!(
        json["error"]["code"].as_str().is_some(),
        "should have string error code"
    );
    assert!(
        json["error"]["message"].as_str().is_some(),
        "should have message"
    );
}

/// On Windows, `.Platform$GUI` must be `"arf-console"` so that packages
/// checking for `"Rgui"` do not call Windows-GUI-only functions.
#[cfg(windows)]
#[test]
fn test_platform_gui_windows() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval(r#".Platform$GUI"#)
        .expect("eval should run");
    assert!(
        result.success,
        "eval should succeed. stderr: {}",
        result.stderr
    );
    // ipc_eval returns raw JSON; parse it and check the `value` field to avoid
    // issues with JSON-escaped quotes (e.g. `\"arf-console\"` vs `"arf-console"`).
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["value"].as_str(),
        Some(r#"[1] "arf-console""#),
        r#".Platform$GUI should be "arf-console", got: {}"#,
        result.stdout
    );
}

/// On non-Windows, `.Platform$GUI` must not be `"arf-console"`: the
/// Windows-only override must not apply on other platforms.
#[cfg(not(windows))]
#[test]
fn test_platform_gui_non_windows() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval(r#".Platform$GUI"#)
        .expect("eval should run");
    assert!(
        result.success,
        "eval should succeed. stderr: {}",
        result.stderr
    );
    // ipc_eval returns raw JSON; parse it and check the `value` field to avoid
    // issues with JSON-escaped quotes.
    let json = parse_ipc_json(&result);
    let value = json["value"]
        .as_str()
        .expect(".Platform$GUI eval should return a non-null string value");
    assert_ne!(
        value, r#"[1] "arf-console""#,
        r#".Platform$GUI must not be "arf-console" on non-Windows, got: {}"#,
        result.stdout
    );
}

/// `system()` must succeed in headless mode.
///
/// On Windows this is a regression test for the `.Platform$GUI` override
/// introduced in GH#168: verifies that `CharacterMode` still works correctly
/// after initialization.
///
/// Uses `Rscript --version` via `R.home("bin")` as a guaranteed cross-platform
/// executable (avoids relying on `echo` as a shell builtin).
#[test]
fn test_system_works() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval(
            r#"system(paste(shQuote(file.path(R.home("bin"), "Rscript")), "--version"), ignore.stdout = TRUE, ignore.stderr = TRUE) == 0L"#,
        )
        .expect("eval should run");
    assert!(
        result.success,
        "eval should succeed. stderr: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["value"].as_str(),
        Some("[1] TRUE"),
        "system(Rscript --version) should return exit code 0, got: {}",
        result.stdout
    );
}

/// Test that `--slave` (a global CLI flag) is accepted without crashing.
///
/// `--slave` is a global flag that must be placed before the subcommand
/// (`arf --slave headless`). In headless mode it is currently ignored, so
/// this test verifies that the flag does not prevent IPC from working.
#[test]
fn test_headless_slave_flag() {
    let process =
        HeadlessProcess::spawn_with_pre_args(&["--slave"]).expect("Failed to spawn with --slave");

    let result = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(
        result.success,
        "IPC eval should work with --slave: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["value"].as_str(),
        Some("[1] 2"),
        "should return result: {}",
        result.stdout
    );
}

/// Test that `--no-echo` (a global CLI flag) is accepted without crashing.
///
/// Like `--slave`, `--no-echo` must precede the subcommand. In headless mode
/// it is currently ignored, so this test verifies that the flag does not
/// prevent IPC from working.
#[test]
fn test_headless_no_echo_flag() {
    let process = HeadlessProcess::spawn_with_pre_args(&["--no-echo"])
        .expect("Failed to spawn with --no-echo");

    let result = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(
        result.success,
        "IPC eval should work with --no-echo: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["value"].as_str(),
        Some("[1] 2"),
        "should return result: {}",
        result.stdout
    );
}

/// Test that a large R value (1000 elements) is returned completely.
///
/// Uses a pure expression (`1:1000`) instead of `print(1:1000)` so this test
/// only validates `value` transport, not stdout capture behavior.
#[test]
fn test_headless_large_output() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process.ipc_eval("1:1000").expect("eval should run");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    let json = parse_ipc_json(&result);
    let value = json["value"]
        .as_str()
        .expect("value should be present for 1:1000");

    // Count numeric tokens (integers 1-1000); index markers like "[16]" are
    // not parseable as u32, so exactly 1000 tokens should parse successfully.
    let count = value
        .split_whitespace()
        .filter(|s| s.parse::<u32>().is_ok())
        .count();
    assert_eq!(
        count, 1000,
        "output should contain exactly 1000 integers (got {count}): {}",
        value
    );
}

/// Test that `message()` output is captured in the `stderr` JSON field.
///
/// `message()` writes to R's stderr stream (WriteConsoleEx type=1), so it
/// should appear in the `stderr` field of the evaluate result, not in
/// `stdout` or `value`.
#[test]
fn test_headless_message_capture() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval(r#"message("hello from message"); invisible(NULL)"#)
        .expect("eval should run");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    let json = parse_ipc_json(&result);
    assert!(
        json["stderr"]
            .as_str()
            .is_some_and(|s| s.contains("hello from message")),
        "message() output should appear in stderr field: {}",
        result.stdout
    );
    assert!(
        json["stdout"].as_str().is_none_or(|s| s.is_empty()),
        "message() should not appear in stdout: {}",
        result.stdout
    );
    assert!(
        json["value"].as_str().is_none(),
        "message() should not produce a value: {}",
        result.stdout
    );
}

/// Test that `warning()` output is captured in the `stderr` JSON field.
///
/// With `options(warn = 1)`, warnings are emitted immediately via
/// WriteConsoleEx type=1 (stderr), so they appear in the `stderr` field.
#[test]
fn test_headless_warning_capture() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval(r#"options(warn = 1); warning("test warning"); invisible(NULL)"#)
        .expect("eval should run");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    let json = parse_ipc_json(&result);
    assert!(
        json["stderr"]
            .as_str()
            .is_some_and(|s| s.contains("test warning")),
        "warning() output should appear in stderr field: {}",
        result.stdout
    );
    assert!(
        json["stdout"].as_str().is_none_or(|s| s.is_empty()),
        "warning() should not appear in stdout: {}",
        result.stdout
    );
    assert!(
        json["value"].as_str().is_none(),
        "warning() should not produce a value: {}",
        result.stdout
    );
}

/// Test that UTF-8 multibyte characters are handled correctly.
///
/// Verifies that R's string operations on multibyte characters produce
/// correct results, and that `cat()` output containing multibyte characters
/// is captured without corruption in the `stdout` field.
#[test]
fn test_headless_utf8_multibyte() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // nchar() should count Unicode characters, not bytes
    let result = process
        .ipc_eval(r#"nchar("日本語")"#)
        .expect("eval should run");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["value"].as_str(),
        Some("[1] 3"),
        "nchar of 3 Japanese characters should be 3: {}",
        result.stdout
    );

    // cat() with multibyte characters should appear in stdout field intact
    let result2 = process
        .ipc_eval(r#"cat("日本語\n")"#)
        .expect("eval should run");
    assert!(result2.success, "eval should succeed: {}", result2.stderr);
    let json2 = parse_ipc_json(&result2);
    assert!(
        json2["stdout"]
            .as_str()
            .is_some_and(|s| s.contains("日本語")),
        "cat() multibyte output should appear in stdout field intact: {}",
        result2.stdout
    );
    assert!(
        json2["stderr"].as_str().is_none_or(|s| s.is_empty()),
        "cat() should not write to stderr: {}",
        result2.stdout
    );
    assert!(
        json2["value"].as_str().is_none(),
        "cat() should not produce a value: {}",
        result2.stdout
    );
}

/// Test that `.libPaths()` returns valid, accessible directories.
///
/// Verifies that R's library search path is non-empty and all returned paths
/// exist on disk. For non-macOS, also verifies that `R.home("library")` is
/// included in `.libPaths()`. For macOS, validates that `R.home("library")`
/// exists on disk (runner setups can legitimately exclude it from `.libPaths()`).
#[test]
fn test_headless_lib_paths_valid() {
    // Run under --vanilla so user/site startup profiles cannot customize
    // library paths and make this invariant environment-dependent.
    let process =
        HeadlessProcess::spawn_with_args(&["--vanilla"]).expect("Failed to spawn headless");

    // .libPaths() must be non-empty
    let result = process
        .ipc_eval("length(.libPaths()) > 0L")
        .expect("eval should run");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["value"].as_str(),
        Some("[1] TRUE"),
        ".libPaths() should not be empty: {}",
        result.stdout
    );

    // Every path returned by .libPaths() must exist on disk
    let result2 = process
        .ipc_eval("all(dir.exists(.libPaths()))")
        .expect("eval should run");
    assert!(result2.success, "eval should succeed: {}", result2.stderr);
    let json2 = parse_ipc_json(&result2);
    assert_eq!(
        json2["value"].as_str(),
        Some("[1] TRUE"),
        "all .libPaths() entries should exist on disk: {}",
        result2.stdout
    );

    #[cfg(not(target_os = "macos"))]
    let result3 = process
        .ipc_eval(r#"R.home("library") %in% .libPaths()"#)
        .expect("eval should run");
    #[cfg(target_os = "macos")]
    let result3 = process
        .ipc_eval(r#"dir.exists(R.home("library"))"#)
        .expect("eval should run");
    assert!(result3.success, "eval should succeed: {}", result3.stderr);
    let json3 = parse_ipc_json(&result3);
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        json3["value"].as_str(),
        Some("[1] TRUE"),
        r#"R.home("library") should be in .libPaths(): {}"#,
        result3.stdout
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        json3["value"].as_str(),
        Some("[1] TRUE"),
        r#"R.home("library") should exist on disk: {}"#,
        result3.stdout
    );
}
