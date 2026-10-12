use super::*;

// ============================================================================
// Script File Execution Tests
// ============================================================================

/// Test running a script file with state across statements and a function.
#[test]
fn test_script_file() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    write!(
        file,
        "x <- 5\ny <- 10\nx + y\nf <- function(x) {{\n  x + 1\n}}\nf(10)\n"
    )
    .expect("Failed to write");

    let output = sanitized_arf_command()
        .arg("-f")
        .arg(file.path())
        .output()
        .expect("Failed to run arf with script file");

    assert!(output.status.success(), "arf -f script.R should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    for (case, expected) in [
        ("statements share state", "[1] 15"),
        ("function definition and call", "[1] 11"),
    ] {
        assert!(
            stdout.contains(expected),
            "{case}: should output {expected:?}: {stdout}"
        );
    }
}

/// Test script file with reprex mode.
/// Verifies that source code is echoed before output in reprex format.
#[test]
fn test_script_file_reprex() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    writeln!(file, "1 + 1").expect("Failed to write");

    let output = sanitized_arf_command()
        .args(["--reprex", "on"])
        .arg("-f")
        .arg(file.path())
        .output()
        .expect("Failed to run arf --reprex -f script.R");

    assert!(
        output.status.success(),
        "arf --reprex -f script.R should succeed"
    );

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

/// Test that CRLF line endings are accepted and converted like Rscript does.
#[test]
fn test_script_file_crlf() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    file.write_all(b"x <- \"a\r\nb\"\r\nprint(charToRaw(x))\r\n")
        .expect("Failed to write");

    let output = sanitized_arf_command()
        .arg("-f")
        .arg(file.path())
        .output()
        .expect("Failed to run arf with CRLF script file");

    assert!(
        output.status.success(),
        "arf -f with CRLF script should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 61 0a 62"),
        "CRLF inside a string should become LF: {}",
        stdout
    );
}

/// Test that a script with a parse error exits with a failure status.
#[test]
fn test_script_file_parse_error_fails() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    writeln!(file, "1 +* 2").expect("Failed to write");

    let output = sanitized_arf_command()
        .arg("-f")
        .arg(file.path())
        .output()
        .expect("Failed to run arf with invalid script file");

    assert_eq!(
        output.status.code(),
        Some(1),
        "arf -f should exit with status 1 on parse errors: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test non-existent script file.
#[test]
fn test_script_file_not_found() {
    let output = sanitized_arf_command()
        .arg("-f")
        .arg("/nonexistent/path/to/script.R")
        .output()
        .expect("Failed to run arf");

    assert!(
        !output.status.success(),
        "arf should fail for non-existent file"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to read") || stderr.contains("No such file"),
        "Should show file not found error: {}",
        stderr
    );
}

/// Test that `arf -f -` reads R code from stdin and, in reprex mode, echoes
/// source and prefixes output.
#[test]
fn test_script_file_stdin_reprex() {
    let mut child = sanitized_arf_command()
        .args(["--reprex", "on", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn arf");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"x <- 7\ny <- 8\nx + y\n")
        .expect("Failed to write to stdin");

    let output = child.wait_with_output().expect("Failed to wait for arf");

    assert!(
        output.status.success(),
        "arf --reprex -f - should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for (case, expected) in [
        ("stdin source is echoed", "x + y"),
        ("stdin script output is prefixed with #>", "#> [1] 15"),
    ] {
        assert!(
            stdout.contains(expected),
            "{case}: output should contain {expected:?}: {stdout}"
        );
    }
}
