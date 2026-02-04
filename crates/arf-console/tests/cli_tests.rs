//! Non-interactive CLI integration tests for arf.
//!
//! These tests cover non-interactive modes: version/help/completions,
//! history schema/import, eval (-e), script file execution, and R completion functions.
//! All tests use `std::process::Command` and work on all platforms.

use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

// ============================================================================
// CLI Tests (non-interactive)
// ============================================================================

/// Test that arf binary exists and can show version.
#[test]
fn test_version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .arg("--version")
        .output()
        .expect("Failed to run arf");

    assert!(output.status.success(), "arf --version should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("arf") || stdout.contains("0.1.0"),
        "Version output should contain version info: {}",
        stdout
    );
}

/// Test that arf binary can show help.
#[test]
fn test_help_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .arg("--help")
        .output()
        .expect("Failed to run arf");

    assert!(output.status.success(), "arf --help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--reprex") && stdout.contains("--no-banner") && stdout.contains("--eval"),
        "Help should show CLI options: {}",
        stdout
    );
}

/// Test shell completion generation.
#[test]
fn test_completions_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["completions", "bash"])
        .output()
        .expect("Failed to run arf completions");

    assert!(
        output.status.success(),
        "arf completions bash should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("complete") || stdout.contains("arf"),
        "Completion output should contain bash completion code: {}",
        stdout
    );
}

/// Test `arf history schema` subcommand displays schema information.
#[test]
fn test_history_schema_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["history", "schema"])
        .output()
        .expect("Failed to run arf history schema");

    assert!(output.status.success(), "arf history schema should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check for expected sections
    assert!(
        stdout.contains("# History Database"),
        "Should contain title: {}",
        stdout
    );
    assert!(
        stdout.contains("## Location"),
        "Should contain location section: {}",
        stdout
    );
    assert!(
        stdout.contains("## SQLite Schema"),
        "Should contain schema section: {}",
        stdout
    );
    assert!(
        stdout.contains("## Indexes"),
        "Should contain indexes section: {}",
        stdout
    );
    assert!(
        stdout.contains("## Analyze or Export"),
        "Should contain export section: {}",
        stdout
    );

    // Check for schema content
    assert!(
        stdout.contains("CREATE TABLE history"),
        "Should contain CREATE TABLE: {}",
        stdout
    );
    assert!(
        stdout.contains("command_line"),
        "Should contain command_line column: {}",
        stdout
    );

    // Check for R example
    assert!(
        stdout.contains("library(DBI)"),
        "Should contain R DBI example: {}",
        stdout
    );
    assert!(
        stdout.contains("dbConnect"),
        "Should contain dbConnect: {}",
        stdout
    );
}

/// Test `arf history schema` outputs plain text when piped.
#[test]
fn test_history_schema_piped_no_colors() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["history", "schema"])
        .output()
        .expect("Failed to run arf history schema");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    // When piped (not a TTY), output should not contain ANSI escape codes
    assert!(
        !stdout.contains("\x1b["),
        "Piped output should not contain ANSI escape codes: {:?}",
        &stdout[..stdout.len().min(200)]
    );
}

/// Test `arf history import --from arf` rejects self-import for r.db (source == target).
#[test]
fn test_history_import_rejects_self_import_r_db() {
    use reedline::SqliteBackedHistory;
    use tempfile::TempDir;

    // Create a temporary history directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let history_dir = temp_dir.path();

    // Create an r.db file using reedline's SqliteBackedHistory
    let r_db_path = history_dir.join("r.db");
    let _db = SqliteBackedHistory::with_file(r_db_path.clone(), None, None)
        .expect("Failed to create r.db");
    drop(_db); // Close the database

    // Try to import from r.db into the same directory's r.db
    // Note: --history-dir is a top-level option, must come before subcommand
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args([
            "--history-dir",
            history_dir.to_str().unwrap(),
            "history",
            "import",
            "--from",
            "arf",
            "--file",
            r_db_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run arf history import");

    // Should fail with self-import error
    assert!(
        !output.status.success(),
        "Self-import should fail, but succeeded"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Refusing to import") && stderr.contains("into itself"),
        "Error should mention refusing self-import, got: {}",
        stderr
    );
}

/// Test `arf history import --from arf` rejects self-import for shell.db (source == target).
#[test]
fn test_history_import_rejects_self_import_shell_db() {
    use reedline::SqliteBackedHistory;
    use tempfile::TempDir;

    // Create a temporary history directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let history_dir = temp_dir.path();

    // Create a shell.db file using reedline's SqliteBackedHistory
    let shell_db_path = history_dir.join("shell.db");
    let _db = SqliteBackedHistory::with_file(shell_db_path.clone(), None, None)
        .expect("Failed to create shell.db");
    drop(_db); // Close the database

    // Try to import from shell.db into the same directory's shell.db
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args([
            "--history-dir",
            history_dir.to_str().unwrap(),
            "history",
            "import",
            "--from",
            "arf",
            "--file",
            shell_db_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run arf history import");

    // Should fail with self-import error
    assert!(
        !output.status.success(),
        "Self-import of shell.db should fail, but succeeded"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Refusing to import") && stderr.contains("into itself"),
        "Error should mention refusing self-import for shell.db, got: {}",
        stderr
    );
}

/// Test `arf history import --from arf` requires --file option.
#[test]
fn test_history_import_arf_requires_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["history", "import", "--from", "arf"])
        .output()
        .expect("Failed to run arf history import");

    // Should fail because --file is required for arf format
    assert!(
        !output.status.success(),
        "Import from arf without --file should fail"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--file") && stderr.contains("required"),
        "Error should mention --file is required, got: {}",
        stderr
    );
}

// ============================================================================
// Script Execution Mode Tests (-e flag)
// ============================================================================

/// Test basic R evaluation with -e flag.
#[test]
fn test_eval_basic() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["-e", "1 + 1"])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[1] 2"), "Should output [1] 2: {}", stdout);
}

/// Test multiple expressions with -e flag.
#[test]
fn test_eval_multiple_expressions() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
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

/// Test that R errors are handled gracefully.
#[test]
fn test_eval_error_handling() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["--reprex", "-e", "1 + 1"])
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
        r###"[mode.reprex]
comment = "## "
"###
    )
    .expect("Failed to write config file");

    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args([
            "--config",
            config_file.path().to_str().unwrap(),
            "--reprex",
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
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["--reprex", "-e", r#"cat("hello")"#])
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
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["--reprex", "-e", r#"cat("hello\n")"#])
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

    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .arg(file.path())
        .output()
        .expect("Failed to run arf with script file");

    assert!(output.status.success(), "arf script.R should succeed");

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

    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .arg(file.path())
        .output()
        .expect("Failed to run arf with script file");

    assert!(output.status.success(), "arf script.R should succeed");

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

    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .arg("--reprex")
        .arg(file.path())
        .output()
        .expect("Failed to run arf --reprex script.R");

    assert!(
        output.status.success(),
        "arf --reprex script.R should succeed"
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
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
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

// ============================================================================
// R Completion Tests (using R's internal completion functions)
// ============================================================================

/// Test that R's completion functions work.
#[test]
fn test_r_completion_functions() {
    // Test that utils completion functions are available
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args([
            "-e",
            r#"
            utils:::.assignLinebuffer("pri")
            utils:::.assignEnd(3)
            token <- utils:::.guessTokenFromLine()
            print(token)
        "#,
        ])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pri"), "Token should be 'pri': {}", stdout);
}

/// Test that R's completeToken works.
#[test]
fn test_r_complete_token() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args([
            "-e",
            r#"
            utils:::.assignLinebuffer("prin")
            utils:::.assignEnd(4L)
            utils:::.guessTokenFromLine()
            utils:::.completeToken()
            comps <- utils:::.retrieveCompletions()
            print(comps)
        "#,
        ])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain "print" in completions
    assert!(
        stdout.contains("print"),
        "Completions should include 'print': {}",
        stdout
    );
}
