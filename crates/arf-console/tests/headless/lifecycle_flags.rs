use super::support::*;
use std::time::Duration;

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
