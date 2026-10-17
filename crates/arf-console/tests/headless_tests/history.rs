use super::support::*;
use crate::common::{poll_until, wait_for_history_rows};
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
    let immediate_history = process
        .ipc_history(&["--grep", "1 + 1"])
        .expect("query history after evaluation response");
    assert!(immediate_history.success, "history query should succeed");
    let immediate_history = parse_ipc_json(&immediate_history);
    assert_eq!(immediate_history["entries"][0]["exit_status"], 0);

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

    // A trailing sentinel proves everything submitted before it was flushed,
    // including the whitespace-only commands checked below.
    let sentinel = process
        .ipc_eval("history_sentinel <- 1")
        .expect("sentinel eval should run");
    assert!(sentinel.success, "sentinel eval should succeed");

    // Read the history database directly once the sentinel is visible.
    let db_path = tmp.path().join("r.db");
    let rows = wait_for_history_rows(
        &db_path,
        &format!(
            "the history database to contain the sentinel: {}",
            db_path.display()
        ),
        Duration::from_secs(10),
        Duration::from_millis(50),
        |rows| rows.iter().any(|r| r.command == "history_sentinel <- 1"),
    )
    .unwrap_or_else(|error| panic!("{error:#}"));

    // Filter to just our known commands for assertion stability.
    let success_row = rows.iter().find(|r| r.command == "1 + 1");
    let error_row = rows.iter().find(|r| r.command == "stop('test_error')");
    let send_row = rows.iter().find(|r| r.command == "invisible(NULL)");

    // Successful eval
    let success_row = success_row.expect("should find '1 + 1' in history");
    assert_eq!(
        success_row.exit_status,
        Some(0),
        "successful eval should have exit_status=0"
    );
    assert!(
        success_row.hostname.is_some(),
        "hostname should be populated"
    );
    assert!(success_row.cwd.is_some(), "cwd should be populated");

    // Error eval
    let error_row = error_row.expect("should find error command in history");
    assert_eq!(
        error_row.exit_status,
        Some(1),
        "error eval should have exit_status=1"
    );

    // user_input (send)
    let send_row = send_row.expect("should find send command in history");
    assert_eq!(
        send_row.exit_status,
        Some(0),
        "send should have exit_status=0"
    );

    // Whitespace-only commands should not appear in history.
    let whitespace_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.command.trim().is_empty())
        .collect();
    assert!(
        whitespace_rows.is_empty(),
        "whitespace-only commands should not be persisted to history"
    );
}

#[test]
fn test_headless_history_captures_working_directory_when_execution_starts() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().join("history");
    let initial_dir = tmp.path().join("initial");
    let work_dir = tmp.path().join("work");
    let gate_path = tmp.path().join("continue-request");
    std::fs::create_dir_all(&history_dir).expect("create history directory");
    std::fs::create_dir_all(&initial_dir).expect("create initial directory");
    std::fs::create_dir_all(&work_dir).expect("create work directory");
    let initial_cwd = initial_dir
        .canonicalize()
        .expect("canonicalize initial directory");
    let work_dir_r = serde_json::to_string(&work_dir.to_string_lossy().as_ref())
        .expect("encode work directory as an R string");
    let gate_path_r = serde_json::to_string(&gate_path.to_string_lossy().as_ref())
        .expect("encode gate path as an R string");

    let process = HeadlessProcess::spawn_with_args_in_dir(
        &[
            "--history-dir",
            history_dir.to_str().expect("history path is UTF-8"),
        ],
        &initial_dir,
    )
    .expect("Failed to spawn headless with --history-dir");

    let command = format!(
        "local({{ deadline <- Sys.time() + 10; while (!file.exists({gate_path_r}) && Sys.time() < deadline) Sys.sleep(0.01); if (!file.exists({gate_path_r})) stop('history test gate timed out'); setwd({work_dir_r}) }})"
    );
    let accepted = process.ipc_send(&command).expect("send should be accepted");
    assert!(accepted.success, "send should be accepted");
    assert_eq!(parse_ipc_json(&accepted)["accepted"], true);

    let db_path = history_dir.join("r.db");
    let pending_rows = wait_for_history_rows(
        &db_path,
        "pending history row for running setwd request",
        Duration::from_secs(5),
        Duration::from_millis(25),
        |rows| {
            rows.iter()
                .any(|row| row.command == command && row.exit_status.is_none())
        },
    )
    .unwrap_or_else(|error| panic!("{error:#}"));
    let pending = pending_rows
        .iter()
        .find(|row| row.command == command)
        .expect("running setwd request should already be in history");
    assert_eq!(pending.cwd.as_deref(), initial_cwd.to_str());
    let timestamp: Option<i64> = rusqlite::Connection::open(&db_path)
        .expect("open history database")
        .query_row(
            "SELECT start_timestamp FROM history WHERE command_line = ?1",
            [&command],
            |row| row.get(0),
        )
        .expect("read start timestamp");
    assert!(
        timestamp.is_some(),
        "start timestamp should be set at begin"
    );
    std::fs::write(&gate_path, "continue").expect("release R request gate");

    let sentinel = process
        .ipc_eval("history_cwd_sentinel <- 1")
        .expect("sentinel should flush history");
    assert!(sentinel.success, "history sentinel should succeed");
    let rows = wait_for_history_rows(
        &db_path,
        "history row for setwd request",
        Duration::from_secs(10),
        Duration::from_millis(50),
        |rows| {
            rows.iter()
                .any(|row| row.command == command && row.exit_status.is_some())
        },
    )
    .unwrap_or_else(|error| panic!("{error:#}"));
    let row = rows
        .iter()
        .find(|row| row.command == command)
        .expect("setwd request should be in history");
    assert_eq!(row.cwd.as_deref(), initial_cwd.to_str());
    assert_eq!(row.exit_status, Some(0));
    assert_eq!(rows.iter().filter(|row| row.command == command).count(), 1);
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
    assert!(
        poll_until(Duration::from_secs(5), Duration::from_millis(50), || {
            (!process.stdout_output().trim().is_empty()).then_some(())
        })
        .is_some(),
        "Timed out waiting for JSON on stdout"
    );

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
    assert!(json["ipc_policy"].is_object());
    assert!(json["ipc_policy"]["silent"].is_object());
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

/// Exercise the `arf ipc history` query options against one headless session
/// and one shared set of evaluated commands: default scope, `--limit`,
/// `--grep`, `--since`, `--cwd`, and entry metadata.
#[test]
fn test_ipc_history_queries() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process =
        HeadlessProcess::spawn_with_args(&["--history-dir", history_dir]).expect("spawn headless");

    let commands = [
        "1 + 1",
        "cat('hello')",
        "print('apple')",
        "cat('banana')",
        "print('apricot')",
        "stop('oops')",
    ];
    for code in commands {
        // R errors are returned in the JSON response, so every eval succeeds.
        let result = process.ipc_eval(code).expect("eval should run");
        assert!(result.success, "eval of {code:?} should succeed");
    }

    // History is written asynchronously: wait until the last command is
    // visible, which implies all earlier ones are too.
    let default_json = wait_for_history_until(
        &process,
        &[],
        Duration::from_secs(5),
        Duration::from_millis(50),
        |json| {
            json["entries"].as_array().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|e| e["command"].as_str() == Some("stop('oops')"))
            })
        },
    );
    let entries = default_json["entries"]
        .as_array()
        .expect("entries should be array");
    let recorded: Vec<&str> = entries
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();

    // Default query: every command, scoped to the current session.
    for code in commands {
        assert!(
            recorded.contains(&code),
            "default query: should contain {code:?}: {recorded:?}"
        );
    }
    let session_id = default_json["session_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("default query: should have numeric session_id: {default_json}"));
    for entry in entries {
        assert_eq!(
            entry["session_id"].as_i64(),
            Some(session_id),
            "default query: all entries should match session_id: {entry}"
        );
    }

    // Metadata.
    let success_entry = entries
        .iter()
        .find(|e| e["command"].as_str() == Some("1 + 1"))
        .expect("metadata: should find success entry");
    assert!(
        success_entry["timestamp"].is_string(),
        "metadata: should have timestamp: {success_entry}"
    );
    assert!(
        success_entry["cwd"].is_string(),
        "metadata: should have cwd: {success_entry}"
    );
    assert_eq!(
        success_entry["exit_status"].as_i64(),
        Some(0),
        "metadata: success should have exit_status=0: {success_entry}"
    );
    let error_entry = entries
        .iter()
        .find(|e| e["command"].as_str() == Some("stop('oops')"))
        .expect("metadata: should find error entry");
    assert_eq!(
        error_entry["exit_status"].as_i64(),
        Some(1),
        "metadata: error should have exit_status=1: {error_entry}"
    );

    // --limit.
    let limited = process
        .ipc_history(&["--limit", "2"])
        .expect("history limit");
    assert!(limited.success, "limit: {}", limited.stderr);
    let limited_json = parse_ipc_json(&limited);
    assert_eq!(
        limited_json["entries"]
            .as_array()
            .expect("entries array")
            .len(),
        2,
        "limit: should return exactly 2 entries: {limited_json}"
    );

    // --grep.
    let grep = process
        .ipc_history(&["--grep", "apple"])
        .expect("history grep");
    assert!(grep.success, "grep: {}", grep.stderr);
    let grep_json = parse_ipc_json(&grep);
    let grep_commands: Vec<&str> = grep_json["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();
    assert!(
        grep_commands.contains(&"print('apple')"),
        "grep: should find the apple command: {grep_commands:?}"
    );
    assert!(
        grep_commands.iter().all(|c| c.contains("apple")),
        "grep: all results should contain 'apple': {grep_commands:?}"
    );

    // --since: the timestamps are far from both bounds, so no sleep is needed.
    let future = process
        .ipc_history(&["--since", "2999-01-01"])
        .expect("history since future");
    assert!(future.success, "since future: {}", future.stderr);
    let future_json = parse_ipc_json(&future);
    assert!(
        future_json["entries"]
            .as_array()
            .expect("entries array")
            .is_empty(),
        "since: a future bound should return no entries: {future_json}"
    );
    let past = process
        .ipc_history(&["--since", "1970-01-01"])
        .expect("history since past");
    assert!(past.success, "since past: {}", past.stderr);
    let past_json = parse_ipc_json(&past);
    assert!(
        past_json["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .any(|e| e["command"].as_str() == Some("1 + 1")),
        "since: a past bound should include the evaluated commands: {past_json}"
    );

    // --cwd exact match.
    let cwd = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let cwd_match = process
        .ipc_history(&["--cwd", &cwd])
        .expect("history cwd match");
    assert!(cwd_match.success, "cwd match: {}", cwd_match.stderr);
    let cwd_match_json = parse_ipc_json(&cwd_match);
    assert!(
        cwd_match_json["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .any(|e| e["command"].as_str() == Some("1 + 1") && e["cwd"].as_str() == Some(&cwd)),
        "cwd: a matching directory should return the commands: {cwd_match_json}"
    );
    let cwd_miss = process
        .ipc_history(&["--cwd", "/definitely/nonexistent/cwd"])
        .expect("history cwd miss");
    assert!(cwd_miss.success, "cwd miss: {}", cwd_miss.stderr);
    let cwd_miss_json = parse_ipc_json(&cwd_miss);
    assert!(
        cwd_miss_json["entries"]
            .as_array()
            .expect("entries array")
            .is_empty(),
        "cwd: a different directory should return no entries: {cwd_miss_json}"
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

/// Volatile history remains queryable in-process but is never written to disk.
#[test]
fn test_ipc_history_volatile() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let history_dir = tmp.path().to_str().unwrap();

    let process = HeadlessProcess::spawn_with_args(&["--history-dir", history_dir, "--no-history"])
        .expect("spawn headless");

    process.ipc_eval("1 + 1").expect("eval should run");
    let result = process.ipc_history(&[]).expect("history query");
    // The volatile in-memory owner is available for the lifetime of the process.
    assert!(
        result.success,
        "history should be queryable in volatile mode: stdout={}, stderr={}",
        result.stdout, result.stderr
    );
    assert!(result.stdout.contains("1 + 1"));
}
