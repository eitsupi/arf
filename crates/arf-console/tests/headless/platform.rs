use super::support::*;

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
