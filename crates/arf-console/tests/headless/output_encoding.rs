use super::support::*;
use std::time::Duration;

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

/// Test that large printed output is captured in `stdout` with `--visible`.
#[test]
fn test_headless_large_output_stdout_visible() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval_visible(r#"print(1:1000)"#)
        .expect("eval should run");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    let json = parse_ipc_json(&result);
    let stdout = json["stdout"]
        .as_str()
        .expect("stdout should be present for visible print");

    let count = stdout
        .split_whitespace()
        .filter(|s| s.parse::<u32>().is_ok())
        .count();
    assert_eq!(
        count, 1000,
        "stdout should contain exactly 1000 integers (got {count}): {}",
        stdout
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
/// Uses `\\u` escapes and UTF-8 conversion helpers so it runs in any locale.
#[test]
fn test_headless_utf8_multibyte() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // utf8ToInt() works across locales and validates Unicode code points.
    let result = process
        .ipc_eval(r#"utf8ToInt("\u65e5\u672c\u8a9e")"#)
        .expect("eval should run");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["value"].as_str(),
        Some("[1] 26085 26412 35486"),
        "utf8ToInt should return expected code points: {}",
        result.stdout
    );

    // Validate UTF-8 byte sequence transport through stdout in printable ASCII form.
    let result2 = process
        .ipc_eval(
            r#"cat(paste(sprintf("%02x", as.integer(charToRaw(enc2utf8("\u65e5\u672c\u8a9e")))), collapse = " "))"#,
        )
        .expect("eval should run");
    assert!(result2.success, "eval should succeed: {}", result2.stderr);
    let json2 = parse_ipc_json(&result2);
    let stdout2 = json2["stdout"]
        .as_str()
        .expect("stdout should be present for raw-byte check");
    assert!(
        stdout2.contains("e6 97 a5 e6 9c ac e8 aa 9e"),
        "UTF-8 bytes should match expected sequence in stdout: {}",
        result2.stdout
    );
    assert!(
        json2["stderr"].as_str().is_none_or(|s| s.is_empty()),
        "stderr should be empty for UTF-8 stdout check: {}",
        result2.stdout
    );
    assert!(
        json2["value"].as_str().is_none(),
        "cat() check should not return value: {}",
        result2.stdout
    );
}

/// Test that `ARF_IPC_SESSIONS_DIR` is honoured by the writer (headless) path.
///
/// Spawns `arf headless` with the env var set to a temp directory and
/// verifies that the session file lands there. This exercises the writer
/// path in `write_session`; `test_ipc_exit_code_transport_error` only
/// exercises the reader path with a synthetic session file.
#[test]
fn test_sessions_dir_override_writer() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let sessions_dir = tmp.path().to_str().expect("sessions_dir is valid utf-8");

    let process = HeadlessProcess::spawn_with_sessions_dir(sessions_dir)
        .expect("Failed to spawn headless with sessions dir override");

    // Wait for the session file to appear in the override directory.
    // write_session() is called before the "IPC server listening" message, so
    // the file should already exist by the time spawn_with_sessions_dir returns.
    let session_file = tmp.path().join(format!("{}.json", process.pid));
    let start = std::time::Instant::now();
    loop {
        if session_file.exists() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "session file not found at {} within timeout",
            session_file.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // IPC should work via the same sessions dir override.
    let result = process.ipc_eval("1 + 1").expect("eval should work");
    assert!(result.success, "eval should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("[1] 2"),
        "should return result: {}",
        result.stdout
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
