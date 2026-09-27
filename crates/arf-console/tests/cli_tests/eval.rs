use super::*;

// ============================================================================
// Script Execution Mode Tests (-e flag)
// ============================================================================

/// Test basic R evaluation with -e flag.
#[test]
fn test_eval_basic() {
    let output = sanitized_arf_command()
        .args(["-e", "1 + 1"])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[1] 2"), "Should output [1] 2: {}", stdout);
}

/// Test that a mismatched R_HOME does not prevent startup or evaluation.
#[test]
#[cfg(unix)]
fn test_eval_with_mismatched_r_home() {
    let output = sanitized_arf_command()
        .env("R_HOME", r"/tmp/arf-test-nonexistent-r-4.0.5")
        .args(["-e", r#"1 + 1"#])
        .output()
        .expect("Failed to run arf -e with mismatched R_HOME");

    assert!(
        output.status.success(),
        "arf -e should succeed with a mismatched R_HOME: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 2"),
        "R should evaluate an expression with a mismatched R_HOME: {}",
        stdout
    );
}

/// Test multiple expressions with -e flag.
#[test]
fn test_eval_multiple_expressions() {
    let output = sanitized_arf_command()
        .args(["-e", "x <- 5\nx * 2"])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 10"),
        "Should output [1] 10: {}",
        stdout
    );
}

/// Test function definition and call with -e flag.
#[test]
fn test_eval_function() {
    let output = sanitized_arf_command()
        .args(["-e", "f <- function(x) { x + 1 }\nf(10)"])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 11"),
        "Function should return 11: {}",
        stdout
    );
}

/// Test that R event processing remains available after using a graphics device.
///
/// This verifies that arf can load R's event-processing API, close a graphics
/// device, and continue evaluating code. Actual graphics-window testing
/// requires a display and is outside this non-interactive integration test.
#[test]
fn test_eval_r_event_processing_api() {
    let output = sanitized_arf_command()
        .args([
            "-e",
            r#"
            # Create a simple plot (opens graphics device)
            # On non-interactive systems, this may use a null device
            invisible(plot(1:3, main = "Event Processing Test"))

            # Call dev.off() to close any graphics device
            invisible(dev.off())

            # Verify R is still responsive
            42
        "#,
        ])
        .output()
        .expect("Failed to run arf -e with plot");

    assert!(
        output.status.success(),
        "arf should succeed with plot command. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 42"),
        "R should be responsive after plot: {}",
        stdout
    );
}

/// Test that R errors are handled gracefully.
#[test]
fn test_eval_error_handling() {
    let output = sanitized_arf_command()
        .args(["-e", "stop('Test error')"])
        .output()
        .expect("Failed to run arf -e");

    // Should still exit successfully (R errors are expected behavior)
    assert!(
        output.status.success(),
        "arf -e should succeed even with R errors"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Test error") || stderr.contains("Error"),
        "Should show error message: {}",
        stderr
    );
}

/// Test pipe operator error handling.
#[test]
fn test_eval_pipe_error() {
    let output = sanitized_arf_command()
        .args(["-e", "1 |> 1"])
        .output()
        .expect("Failed to run arf -e");

    assert!(
        output.status.success(),
        "arf -e should succeed even with R errors"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error") || stderr.contains("error"),
        "Should show pipe error: {}",
        stderr
    );
}

/// Test reprex mode with -e flag.
/// Verifies that source code is echoed before output in reprex format.
#[test]
fn test_eval_reprex_mode() {
    let output = sanitized_arf_command()
        .args(["--reprex", "on", "-e", "1 + 1"])
        .output()
        .expect("Failed to run arf --reprex -e");

    assert!(output.status.success(), "arf --reprex -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check that source code is echoed
    assert!(
        stdout.contains("1 + 1"),
        "Output should echo source code: {}",
        stdout
    );
    // Check that result is prefixed with #>
    assert!(
        stdout.contains("#> [1] 2"),
        "Output should be prefixed with #>: {}",
        stdout
    );
}

/// Test custom reprex comment prefix via config file.
/// Verifies that source code is echoed and output uses custom comment prefix.
#[test]
fn test_eval_reprex_custom_comment() {
    // Create a temp config file with custom reprex comment
    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    writeln!(
        config_file,
        r###"[reprex]
comment = "## "
"###
    )
    .expect("Failed to write config file");

    let output = sanitized_arf_command()
        .args([
            "--config",
            config_file.path().to_str().unwrap(),
            "--reprex",
            "on",
            "-e",
            "1 + 1",
        ])
        .output()
        .expect("Failed to run arf --reprex with config file");

    assert!(
        output.status.success(),
        "arf with custom reprex comment should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check that source code is echoed
    assert!(
        stdout.contains("1 + 1"),
        "Output should echo source code: {}",
        stdout
    );
    // Check that result uses custom comment prefix
    assert!(
        stdout.contains("## [1] 2"),
        "Output should be prefixed with custom comment: {}",
        stdout
    );
}

/// Test reprex mode with cat() output.
/// cat() writes to stdout without trailing newline, which should still be captured.
#[test]
fn test_eval_reprex_cat_output() {
    let output = sanitized_arf_command()
        .args(["--reprex", "on", "-e", r#"cat("hello")"#])
        .output()
        .expect("Failed to run arf --reprex -e cat()");

    assert!(
        output.status.success(),
        "arf --reprex -e cat() should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check that source code is echoed
    assert!(
        stdout.contains(r#"cat("hello")"#),
        "Output should echo source code: {}",
        stdout
    );
    // Check that cat() output is captured with reprex comment prefix
    assert!(
        stdout.contains("#> hello"),
        "cat() output should be prefixed with #>: {}",
        stdout
    );
}

/// Test reprex mode with cat() output that includes newline.
#[test]
fn test_eval_reprex_cat_with_newline() {
    let output = sanitized_arf_command()
        .args(["--reprex", "on", "-e", r#"cat("hello\n")"#])
        .output()
        .expect("Failed to run arf --reprex -e cat() with newline");

    assert!(
        output.status.success(),
        "arf --reprex -e cat() with newline should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check that cat() output with newline is captured
    assert!(
        stdout.contains("#> hello"),
        "cat() output with newline should be prefixed with #>: {}",
        stdout
    );
}
