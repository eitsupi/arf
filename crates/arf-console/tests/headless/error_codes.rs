use super::support::*;
use std::process::Command;

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
/// Uses a synthetic session file pointing at a nonexistent socket path.
#[test]
fn test_ipc_exit_code_transport_error() {
    let test_pid = std::process::id();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&session_dir)
        .unwrap_or_else(|e| panic!("create session dir {}: {e}", session_dir.display()));

    #[cfg(unix)]
    let bogus_socket = format!("/tmp/arf-missing-{}.sock", test_pid);
    #[cfg(windows)]
    let bogus_socket = format!(r"\\.\pipe\arf-missing-{}", test_pid);

    let session_path = session_dir.join(format!("{test_pid}.json"));
    let session_json = serde_json::json!({
        "pid": test_pid,
        "socket_path": bogus_socket,
        "r_version": null,
        "cwd": ".",
        "started_at": "1970-01-01T00:00:00Z",
        "log_file": null,
        "history_session_id": null
    });
    std::fs::write(
        &session_path,
        serde_json::to_string_pretty(&session_json).expect("serialize session file"),
    )
    .unwrap_or_else(|e| panic!("write session file {}: {e}", session_path.display()));

    let pid_arg = test_pid.to_string();
    let env_overrides = vec![(
        "ARF_IPC_SESSIONS_DIR",
        session_dir
            .to_str()
            .expect("session_dir must be valid utf-8"),
    )];

    let output = run_ipc_command_with_env(&["ipc", "eval", "1", "--pid", &pid_arg], &env_overrides);
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
