use super::support::*;
use std::process::Command;

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

/// Test that an interactive-only top-level `--slave` flag is rejected before
/// the headless subcommand instead of being silently ignored.
#[test]
fn test_top_level_slave_before_headless_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["--slave", "headless"])
        .output()
        .expect("Failed to run arf with --slave");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("'--slave' is not used by the 'headless' subcommand"));
}

/// Test that an interactive-only top-level `--no-echo` flag is rejected before
/// the headless subcommand instead of being silently ignored.
#[test]
fn test_top_level_no_echo_before_headless_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["--no-echo", "headless"])
        .output()
        .expect("Failed to run arf with --no-echo");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("'--no-echo' is not used by the 'headless' subcommand"));
}
