use super::*;

// ============================================================================
// Script File Execution Tests
// ============================================================================

/// Test running a script file.
#[test]
fn test_script_file() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    writeln!(file, "x <- 5").expect("Failed to write");
    writeln!(file, "y <- 10").expect("Failed to write");
    writeln!(file, "x + y").expect("Failed to write");

    let output = sanitized_arf_command()
        .arg("-f")
        .arg(file.path())
        .output()
        .expect("Failed to run arf with script file");

    assert!(output.status.success(), "arf -f script.R should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 15"),
        "Script should output [1] 15: {}",
        stdout
    );
}

/// Test script file with function definition.
#[test]
fn test_script_file_function() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    writeln!(file, "f <- function(x) {{").expect("Failed to write");
    writeln!(file, "  x + 1").expect("Failed to write");
    writeln!(file, "}}").expect("Failed to write");
    writeln!(file, "f(10)").expect("Failed to write");

    let output = sanitized_arf_command()
        .arg("-f")
        .arg(file.path())
        .output()
        .expect("Failed to run arf with script file");

    assert!(output.status.success(), "arf -f script.R should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 11"),
        "Function should return 11: {}",
        stdout
    );
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

/// Test that `arf -f -` reads R code from stdin and executes it.
#[test]
fn test_script_file_stdin() {
    let mut child = sanitized_arf_command()
        .args(["-f", "-"])
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
        "arf -f - should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[1] 15"),
        "Script via stdin should output [1] 15: {}",
        stdout
    );
}

/// Test that `arf -f -` in reprex mode echoes source and prefixes output.
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
        .write_all(b"1 + 1\n")
        .expect("Failed to write to stdin");

    let output = child.wait_with_output().expect("Failed to wait for arf");

    assert!(
        output.status.success(),
        "arf --reprex -f - should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 + 1"),
        "Reprex via stdin should echo source code: {}",
        stdout
    );
    assert!(
        stdout.contains("#> [1] 2"),
        "Reprex via stdin should prefix output with #>: {}",
        stdout
    );
}
