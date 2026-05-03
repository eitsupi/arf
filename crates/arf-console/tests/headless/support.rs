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
pub(crate) fn parse_ipc_json(output: &IpcOutput) -> serde_json::Value {
    serde_json::from_str(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "failed to parse JSON: {e}\nstdout: {}\nstderr: {}",
            output.stdout, output.stderr
        )
    })
}

/// Poll `ipc history` until `predicate` returns true or timeout.
pub(crate) fn wait_for_history_until<F>(
    process: &HeadlessProcess,
    extra_args: &[&str],
    timeout: Duration,
    poll_interval: Duration,
    mut predicate: F,
) -> serde_json::Value
where
    F: FnMut(&serde_json::Value) -> bool,
{
    let start = std::time::Instant::now();
    let mut last_json = serde_json::json!({});
    let mut last_error = String::new();
    loop {
        match process.ipc_history(extra_args) {
            Ok(result) if result.success => {
                let json = parse_ipc_json(&result);
                if predicate(&json) {
                    return json;
                }
                last_json = json;
            }
            Ok(result) => {
                last_error = format!("ipc_history returned non-success: {}", result.stderr);
            }
            Err(e) => {
                last_error = format!("ipc_history command failed: {e}");
            }
        }
        if start.elapsed() > timeout {
            panic!(
                "timeout waiting for expected history state; last response: {last_json}; last error: {last_error}"
            );
        }
        std::thread::sleep(poll_interval);
    }
}

/// Run `arf ipc ...` directly and capture output.
pub(crate) fn run_ipc_command(args: &[&str]) -> std::process::Output {
    let bin_path = env!("CARGO_BIN_EXE_arf");
    Command::new(bin_path)
        .args(args)
        .output()
        .expect("run arf ipc")
}

pub(crate) fn run_ipc_command_with_env(
    args: &[&str],
    env_overrides: &[(&str, &str)],
) -> std::process::Output {
    let bin_path = env!("CARGO_BIN_EXE_arf");
    let mut cmd = Command::new(bin_path);
    cmd.args(args);
    for (key, value) in env_overrides {
        cmd.env(key, value);
    }
    cmd.output().expect("run arf ipc")
}

/// Wrapper around a headless arf process.
///
/// Spawns `arf headless` and waits for IPC readiness by monitoring
/// stderr for the "IPC server listening on:" message.
pub(crate) struct HeadlessProcess {
    pub(crate) child: Child,
    pub(crate) pid: u32,
    env_overrides: Vec<(String, String)>,
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
    pub(crate) fn spawn() -> Result<Self, String> {
        Self::spawn_with_args(&[])
    }

    /// Spawn `arf headless` with additional R flags and wait for IPC readiness.
    pub(crate) fn spawn_with_args(extra_args: &[&str]) -> Result<Self, String> {
        Self::spawn_inner(&[], extra_args, &[], None)
    }

    /// Spawn `arf headless` with global flags placed before the subcommand.
    pub(crate) fn spawn_with_pre_args(pre_args: &[&str]) -> Result<Self, String> {
        Self::spawn_inner(pre_args, &[], &[], None)
    }

    /// Spawn with a custom sessions directory (sets `ARF_IPC_SESSIONS_DIR`).
    pub(crate) fn spawn_with_sessions_dir(sessions_dir: &str) -> Result<Self, String> {
        Self::spawn_inner(&[], &[], &[("ARF_IPC_SESSIONS_DIR", sessions_dir)], None)
    }

    /// Spawn with Windows creation flags (e.g., CREATE_NEW_PROCESS_GROUP).
    #[cfg(windows)]
    pub(crate) fn spawn_with_creation_flags(
        extra_args: &[&str],
        flags: u32,
    ) -> Result<Self, String> {
        Self::spawn_inner(&[], extra_args, &[], Some(flags))
    }

    fn spawn_inner(
        pre_subcommand_args: &[&str],
        extra_args: &[&str],
        env_overrides: &[(&str, &str)],
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
        for (key, value) in env_overrides {
            cmd.env(key, value);
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
                let mut probe = Command::new(bin_path);
                probe.args([
                    "ipc",
                    "eval",
                    "1",
                    "--pid",
                    &pid.to_string(),
                    "--timeout",
                    "500",
                ]);
                for (key, value) in env_overrides {
                    probe.env(key, value);
                }
                let probe = probe.output();
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
            env_overrides: env_overrides
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            _stderr_thread: Some(stderr_thread),
            _stdout_thread: Some(stdout_thread),
            shutdown,
            stderr_output,
            stdout_output,
        })
    }

    /// Run `arf ipc eval <code> --pid <pid>` and return (stdout, stderr, success).
    pub(crate) fn ipc_eval(&self, code: &str) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let mut cmd = Command::new(bin_path);
        cmd.args(["ipc", "eval", code, "--pid", &self.pid.to_string()]);
        for (key, value) in &self.env_overrides {
            cmd.env(key, value);
        }
        let output = cmd
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
    pub(crate) fn ipc_eval_visible(&self, code: &str) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let mut cmd = Command::new(bin_path);
        cmd.args([
            "ipc",
            "eval",
            code,
            "--pid",
            &self.pid.to_string(),
            "--visible",
        ]);
        for (key, value) in &self.env_overrides {
            cmd.env(key, value);
        }
        let output = cmd
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
    pub(crate) fn ipc_eval_with_timeout(
        &self,
        code: &str,
        timeout_ms: u64,
    ) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let mut cmd = Command::new(bin_path);
        cmd.args([
            "ipc",
            "eval",
            code,
            "--pid",
            &self.pid.to_string(),
            "--timeout",
            &timeout_ms.to_string(),
        ]);
        for (key, value) in &self.env_overrides {
            cmd.env(key, value);
        }
        let output = cmd
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
    pub(crate) fn ipc_send(&self, code: &str) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let mut cmd = Command::new(bin_path);
        cmd.args(["ipc", "send", code, "--pid", &self.pid.to_string()]);
        for (key, value) in &self.env_overrides {
            cmd.env(key, value);
        }
        let output = cmd
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
    pub(crate) fn ipc_session(&self) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let mut cmd = Command::new(bin_path);
        cmd.args(["ipc", "session", "--pid", &self.pid.to_string()]);
        for (key, value) in &self.env_overrides {
            cmd.env(key, value);
        }
        let output = cmd
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
    pub(crate) fn ipc_history(&self, extra_args: &[&str]) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");
        let pid_str = self.pid.to_string();

        let mut args = vec!["ipc", "history", "--pid", &pid_str];
        args.extend_from_slice(extra_args);

        let mut cmd = Command::new(bin_path);
        cmd.args(&args);
        for (key, value) in &self.env_overrides {
            cmd.env(key, value);
        }
        let output = cmd
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
    pub(crate) fn ipc_shutdown(&self) -> Result<IpcOutput, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let mut cmd = Command::new(bin_path);
        cmd.args(["ipc", "shutdown", "--pid", &self.pid.to_string()]);
        for (key, value) in &self.env_overrides {
            cmd.env(key, value);
        }
        let output = cmd
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
    pub(crate) fn wait_for_exit(
        &mut self,
        timeout: Duration,
    ) -> Result<std::process::ExitStatus, String> {
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
    pub(crate) fn stderr_output(&self) -> String {
        self.stderr_output
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Get the headless process's stdout output collected so far.
    pub(crate) fn stdout_output(&self) -> String {
        self.stdout_output
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Get all output (stdout + stderr) from the headless process.
    pub(crate) fn server_output(&self) -> String {
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
pub(crate) struct IpcOutput {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) success: bool,
    pub(crate) exit_code: Option<i32>,
}

// ===========================================================================
// Tests
// ===========================================================================
