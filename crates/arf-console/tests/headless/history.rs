use super::support::*;
use std::time::Duration;

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

    let past_json = wait_for_history_until(
        &process,
        &["--since", "1970-01-01"],
        Duration::from_secs(5),
        Duration::from_millis(50),
        |json| {
            json["entries"].as_array().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|e| e["command"].as_str() == Some("1 + 1"))
            })
        },
    );
    let past_entries = past_json["entries"].as_array().expect("entries array");
    assert!(!past_entries.is_empty(), "past entries should not be empty");
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

    let cwd = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let match_json = wait_for_history_until(
        &process,
        &["--cwd", &cwd],
        Duration::from_secs(5),
        Duration::from_millis(50),
        |json| {
            json["entries"].as_array().is_some_and(|entries| {
                entries.iter().any(|e| {
                    e["command"].as_str() == Some("1 + 1") && e["cwd"].as_str() == Some(&cwd)
                })
            })
        },
    );
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
    let default_json = wait_for_history_until(
        &p1,
        &[],
        Duration::from_secs(5),
        Duration::from_millis(50),
        |json| {
            json["entries"].as_array().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|e| e["command"].as_str() == Some("cmd_from_p1 <- 1"))
            })
        },
    );
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

    let all_json = wait_for_history_until(
        &p1,
        &["--all-sessions"],
        Duration::from_secs(5),
        Duration::from_millis(50),
        |json| {
            json["entries"].as_array().is_some_and(|entries| {
                let has_p1 = entries
                    .iter()
                    .any(|e| e["command"].as_str() == Some("cmd_from_p1 <- 1"));
                let has_p2 = entries
                    .iter()
                    .any(|e| e["command"].as_str() == Some("cmd_from_p2 <- 2"));
                has_p1 && has_p2
            })
        },
    );
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
