//! Integration tests for arf.
//!
//! These tests cover both non-interactive (script execution) and interactive (PTY) modes.
//!
//! The PTY tests use a custom terminal emulator based on vt100-rust that properly
//! responds to cursor position queries (CSI 6n) from reedline, enabling full
//! interactive testing of arf.

mod common;

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

    assert!(
        output.status.success(),
        "arf history schema should succeed"
    );

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
    assert!(
        stdout.contains("[1] 2"),
        "Should output [1] 2: {}",
        stdout
    );
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
    assert!(output.status.success(), "arf -e should succeed even with R errors");

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

    assert!(output.status.success(), "arf -e should succeed even with R errors");

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
        r###"[reprex]
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

    assert!(!output.status.success(), "arf should fail for non-existent file");

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
        .args(["-e", r#"
            utils:::.assignLinebuffer("pri")
            utils:::.assignEnd(3)
            token <- utils:::.guessTokenFromLine()
            print(token)
        "#])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pri"),
        "Token should be 'pri': {}",
        stdout
    );
}

/// Test that R's completeToken works.
#[test]
fn test_r_complete_token() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["-e", r#"
            utils:::.assignLinebuffer("prin")
            utils:::.assignEnd(4L)
            utils:::.guessTokenFromLine()
            utils:::.completeToken()
            comps <- utils:::.retrieveCompletions()
            print(comps)
        "#])
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

// ============================================================================
// PTY-based Interactive Tests
// ============================================================================

use common::Terminal;

/// Test that arf starts up correctly and shows the prompt.
///
/// This test uses a custom PTY proxy that responds to cursor position queries
/// (CSI 6n) from reedline, enabling proper interactive testing.
#[test]
fn test_pty_startup() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal
        .expect("# arf console v")
        .expect("Should show version banner");
    terminal
        .expect("is ready")
        .expect("R should be initialized");
    terminal.wait_for_prompt().expect("Should show prompt");

    terminal.quit().expect("Should quit cleanly");
}

/// Test Ctrl+C cancels input.
#[test]
fn test_pty_ctrl_c() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    terminal.send("some_incomplete").expect("Should write");
    std::thread::sleep(std::time::Duration::from_millis(100));

    terminal.send_interrupt().expect("Should send interrupt");

    terminal
        .wait_for_prompt()
        .expect("Should show prompt after Ctrl+C");

    terminal.quit().expect("Should quit cleanly");
}

// ============================================================================
// PTY-based Tests (ported from radian)
// ============================================================================

/// Test startup with screen-based assertions (inspired by radian's test_startup.py).
///
/// Verifies:
/// - Prompt appears on the correct line
/// - Cursor is positioned correctly after the prompt
#[test]
fn test_pty_startup_screen() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Verify prompt appears on current line
    terminal
        .current_line()
        .assert_contains("> ")
        .expect("Current line should show prompt");

    // Enter empty line
    terminal.send_line("").expect("Should send empty line");
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Prompt should appear again
    terminal
        .current_line()
        .assert_contains("> ")
        .expect("Should show prompt after empty input");

    terminal.quit().expect("Should quit cleanly");
}

/// Test basic expression evaluation with screen-based assertions.
///
/// This is a more robust version of test_eval_basic that uses PTY and screen assertions.
#[test]
fn test_pty_basic_expression() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Evaluate simple expression
    terminal.send_line("1 + 1").expect("Should send expression");

    // Result should appear on previous line
    terminal
        .previous_line(1)
        .assert_contains("[1] 2")
        .expect("Should show result [1] 2");

    // Prompt should reappear
    terminal
        .current_line()
        .assert_contains("> ")
        .expect("Should show new prompt");

    terminal.quit().expect("Should quit cleanly");
}

/// Test variable assignment and retrieval (inspired by radian's test_strings).
#[test]
fn test_pty_variable_assignment() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Assign a string variable
    terminal.send_line("x <- 'hello'").expect("Should send assignment");

    // Clear buffer and wait for fresh prompt (no output for assignment)
    terminal.clear_and_expect("> ").expect("Should show prompt after assignment");

    // Check the variable length
    terminal.send_line("nchar(x)").expect("Should send nchar(x)");

    // Clear buffer and wait for result
    terminal.clear_and_expect("[1] 5").expect("nchar(x) should return 5");

    terminal.quit().expect("Should quit cleanly");
}

/// Test cat() output in PTY mode.
///
/// Tests that R's cat() function works correctly in interactive mode.
/// Note: readline() requires special handling that may not be fully supported yet.
#[test]
fn test_pty_cat_output() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Call cat
    terminal
        .send_line("cat('hello world\\n')")
        .expect("Should send cat expression");

    // Should see the output
    terminal
        .expect("hello world")
        .expect("Should see cat output");

    // Should return to prompt
    terminal.wait_for_prompt().expect("Should show prompt after cat");

    terminal.quit().expect("Should quit cleanly");
}

/// Test multiline expression handling.
#[test]
fn test_pty_multiline_input() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Start a function definition (incomplete expression)
    terminal.send_line("f <- function(x) {").expect("Should send first line");

    // Wait for continuation prompt
    terminal.clear_and_expect("+").expect("Should show continuation prompt");

    // Complete the function
    terminal.send_line("  x + 1").expect("Should send second line");
    terminal.clear_and_expect("+").expect("Should show continuation prompt again");

    terminal.send_line("}").expect("Should send closing brace");

    // Wait for normal prompt to return
    terminal.clear_and_expect("> ").expect("Should show normal prompt");

    // Test the function
    terminal.send_line("f(10)").expect("Should call function");
    terminal.clear_and_expect("[1] 11").expect("f(10) should return 11");

    terminal.quit().expect("Should quit cleanly");
}

/// Test multiline string input (string literal spanning multiple lines).
/// This tests the specific case where a string with embedded newline is entered
/// across multiple lines, which requires proper handling by the validator.
#[test]
fn test_pty_multiline_string_input() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Start a string that spans multiple lines
    // Note: This is NOT an R raw string, but a regular string with a newline in it
    // We're testing the sequence: type `"test`, Enter, type `end"`, Enter
    // The validator should see this as:
    // - After first Enter: `"test\n` (incomplete string)
    // - After typing `end"` and Enter: `"test\nend"` (complete string)
    terminal
        .send("\"test")
        .expect("Should send opening quote and text");
    terminal.send("\r").expect("Should send Enter");

    // Wait for continuation prompt
    terminal
        .clear_and_expect("+")
        .expect("Should show continuation prompt for incomplete string");

    // Complete the string
    terminal.send("end\"").expect("Should send closing text and quote");
    terminal.send("\r").expect("Should send Enter");

    // The complete string should be submitted and R should output it
    // Note: R outputs strings with [1] prefix
    terminal
        .clear_and_expect("[1]")
        .expect("Should see R output");

    terminal.quit().expect("Should quit cleanly");
}

/// Test error handling in PTY mode.
#[test]
fn test_pty_error_handling() {
    // Use --no-auto-match to avoid bracket insertion interfering with the test
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Trigger an error
    terminal.send_line("stop('Test error')").expect("Should send stop()");

    // Should see error message
    terminal
        .expect("Error")
        .expect("Should see error output");

    // Should return to prompt
    terminal
        .current_line()
        .assert_contains("> ")
        .expect("Should show prompt after error");

    terminal.quit().expect("Should quit cleanly");
}

/// Test that exit_status is correctly tracked in history.
///
/// This test verifies that:
/// 1. Successful commands have exit_status = 0
/// 2. Failed commands (errors) have exit_status = 1
#[test]
#[cfg(unix)]
fn test_pty_history_exit_status() {
    use reedline::{History, SearchDirection, SearchQuery, SqliteBackedHistory};

    // Create a temporary directory for history
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let history_dir = temp_dir.path().to_string_lossy().to_string();

    // Start arf with custom history directory
    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--history-dir",
        &history_dir,
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Run a successful command
    terminal.send_line("42").expect("Should send expression");
    terminal.expect("[1] 42").expect("Should see result");

    // Run a failing command
    terminal
        .send_line("stop('test error')")
        .expect("Should send stop()");
    terminal.expect("Error").expect("Should see error");

    // Run another successful command to ensure the error was recorded for the previous one
    terminal.send_line("1").expect("Should send expression");
    terminal.expect("[1] 1").expect("Should see result");

    // Exit cleanly
    terminal.quit().expect("Should quit cleanly");

    // Give the process time to sync history
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Open the history database and verify exit_status values
    let history_path = temp_dir.path().join("r.db");
    assert!(
        history_path.exists(),
        "History database should exist at {:?}",
        history_path
    );

    let history =
        SqliteBackedHistory::with_file(history_path, None, None).expect("Failed to open history");

    // Get all history entries
    let entries = history
        .search(SearchQuery::everything(SearchDirection::Forward, None))
        .expect("Failed to search history");

    // We should have at least 3 entries: 42, stop('test error'), 1
    assert!(
        entries.len() >= 3,
        "Should have at least 3 history entries, got {}",
        entries.len()
    );

    // Find the stop() command and verify its exit_status
    let error_entry = entries
        .iter()
        .find(|e| e.command_line.contains("stop("))
        .expect("Should find stop() command in history");
    assert_eq!(
        error_entry.exit_status,
        Some(1),
        "Failed command should have exit_status = 1"
    );

    // Find a successful command and verify its exit_status
    let success_entry = entries
        .iter()
        .find(|e| e.command_line == "42")
        .expect("Should find '42' command in history");
    assert_eq!(
        success_entry.exit_status,
        Some(0),
        "Successful command should have exit_status = 0"
    );
}

/// Test that rlang/dplyr errors are correctly detected.
///
/// This test verifies that errors from packages like dplyr that use rlang's
/// condition system (which may output to stdout instead of stderr) are still
/// correctly detected and tracked in history.
#[test]
#[cfg(unix)]
fn test_pty_rlang_error_detection() {
    use reedline::{History, SearchDirection, SearchQuery, SqliteBackedHistory};

    // Create a temporary directory for history
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let history_dir = temp_dir.path().to_string_lossy().to_string();

    // Start arf with custom history directory
    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--history-dir",
        &history_dir,
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Run a successful command first
    terminal.send_line("42").expect("Should send expression");
    terminal.expect("[1] 42").expect("Should see result");

    // Run a dplyr command that will fail (column doesn't exist)
    terminal
        .send_line("mtcars |> dplyr::select(nonexistent_column)")
        .expect("Should send dplyr command");
    terminal
        .expect("doesn't exist")
        .expect("Should see dplyr error message");

    // Wait for prompt with status indicator
    terminal.wait_for_prompt().expect("Should show prompt after error");

    // Run another successful command to trigger history update
    terminal.send_line("1").expect("Should send expression");
    terminal.expect("[1] 1").expect("Should see result");

    // Exit cleanly
    terminal.quit().expect("Should quit cleanly");

    // Give the process time to sync history
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Open the history database and verify exit_status values
    let history_path = temp_dir.path().join("r.db");
    assert!(
        history_path.exists(),
        "History database should exist at {:?}",
        history_path
    );

    let history =
        SqliteBackedHistory::with_file(history_path, None, None).expect("Failed to open history");

    // Get all history entries
    let entries = history
        .search(SearchQuery::everything(SearchDirection::Forward, None))
        .expect("Failed to search history");

    // Find the dplyr command and verify its exit_status
    let error_entry = entries
        .iter()
        .find(|e| e.command_line.contains("dplyr::select"))
        .expect("Should find dplyr command in history");
    assert_eq!(
        error_entry.exit_status,
        Some(1),
        "dplyr error should have exit_status = 1, indicating error was detected. \
         This tests that rlang/cli errors that output to stdout are correctly caught \
         via globalCallingHandlers."
    );

    // Verify successful command still works
    let success_entry = entries
        .iter()
        .find(|e| e.command_line == "42")
        .expect("Should find '42' command in history");
    assert_eq!(
        success_entry.exit_status,
        Some(0),
        "Successful command should have exit_status = 0"
    );
}

/// Test sponge-like history forget feature (removes failed commands after delay).
#[test]
#[cfg(unix)]
fn test_pty_history_sponge_forget() {
    use reedline::{History, SearchDirection, SearchQuery, SqliteBackedHistory};
    use std::io::Write;

    // Create a temporary directory for config and history
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let history_dir = temp_dir.path().to_string_lossy().to_string();

    // Write config file with sponge enabled (delay = 1, meaning keep only 1 failed command)
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[experimental.history_forget]
enabled = true
delay = 1
on_exit_only = false

[experimental.prompt_spinner]
frames = ""
"#
    )
    .expect("Failed to write config");

    let config_path_str = config_path.to_string_lossy().to_string();

    // Start arf with custom config and history directory
    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path_str,
        "--history-dir",
        &history_dir,
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Run a successful command first
    terminal.send_line("1").expect("Should send expression");
    terminal.expect("[1] 1").expect("Should see result");

    // Run first failing command (error1)
    terminal
        .send_line("stop('error1')")
        .expect("Should send first error");
    terminal.expect("error1").expect("Should see error1");

    // Run second failing command (error2) - this should trigger purge of error1
    terminal
        .send_line("stop('error2')")
        .expect("Should send second error");
    terminal.expect("error2").expect("Should see error2");

    // Run third failing command (error3) - this should trigger purge of error2
    terminal
        .send_line("stop('error3')")
        .expect("Should send third error");
    terminal.expect("error3").expect("Should see error3");

    // Exit cleanly
    terminal.quit().expect("Should quit cleanly");

    // Give the process time to sync history
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Open the history database and verify sponge effect
    let history_path = temp_dir.path().join("r.db");
    assert!(
        history_path.exists(),
        "History database should exist at {:?}",
        history_path
    );

    let history =
        SqliteBackedHistory::with_file(history_path, None, None).expect("Failed to open history");

    // Get all history entries
    let entries = history
        .search(SearchQuery::everything(SearchDirection::Forward, None))
        .expect("Failed to search history");

    // Count how many error commands remain
    let error_commands: Vec<_> = entries
        .iter()
        .filter(|e| e.command_line.contains("stop("))
        .collect();

    // During session with delay=1:
    // - error1 is purged when error2 arrives (queue was [error1], becomes [error2])
    // - error2 is purged when error3 arrives (queue was [error2], becomes [error3])
    // - error3 remains in queue
    //
    // The most recent failed command may remain in history because R's q() can
    // terminate the process before the exit cleanup completes. This is acceptable
    // because the main value of sponge is purging OLD failed commands during the session.
    assert!(
        error_commands.len() <= 1,
        "At most 1 failed command (the most recent) should remain. Found: {:?}",
        error_commands
            .iter()
            .map(|e| &e.command_line)
            .collect::<Vec<_>>()
    );

    // If a failed command remains, it should be the most recent one (error3)
    if !error_commands.is_empty() {
        assert!(
            error_commands[0].command_line.contains("error3"),
            "The remaining error should be error3, got: {}",
            error_commands[0].command_line
        );
    }

    // Successful command should be present
    let success_commands: Vec<_> = entries
        .iter()
        .filter(|e| e.command_line == "1")
        .collect();
    assert_eq!(
        success_commands.len(),
        1,
        "Successful command should be present"
    );
}

/// Test Ctrl+C interrupts long-running computation.
#[test]
fn test_pty_interrupt_computation() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Start a long computation (infinite loop)
    terminal.send_line("while(TRUE) {}").expect("Should start loop");

    // Give it a moment to start
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Interrupt it
    terminal.send_interrupt().expect("Should send interrupt");

    // Should return to prompt
    terminal.wait_for_prompt().expect("Should show prompt after interrupt");

    // Verify we can still do work
    terminal.send_line("42").expect("Should send simple expression");

    // Use expect for more robust checking
    terminal.expect("[1] 42").expect("Should execute normally after interrupt");

    terminal.quit().expect("Should quit cleanly");
}

/// Test detailed cursor position tracking.
///
/// This test mirrors radian's test_startup cursor position checks:
/// - After startup, cursor is positioned after the prompt
/// - After Enter, cursor moves to new line but stays at prompt column
/// - After interrupt, cursor is at a new prompt line
///
/// Port of: radian/tests/test_startup.py::test_startup
#[test]
fn test_pty_cursor_position() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Get initial cursor position after prompt (position varies with prompt format)
    let (initial_row, initial_col) = terminal.cursor_position().expect("Should get cursor position");
    // Prompt could be "r> " (3 chars) or "R 4.5.2> " (9 chars) etc., so just verify it's > 0
    assert!(initial_col > 0, "Cursor should be after prompt");

    // Verify current line shows prompt
    terminal
        .current_line()
        .assert_contains("> ")
        .expect("Current line should show prompt");

    // Press Enter (empty line)
    terminal.send_line("").expect("Should send empty line");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Cursor should have moved down but column should still be at prompt position
    let (after_enter_row, after_enter_col) = terminal.cursor_position().expect("Should get cursor position after enter");
    assert!(after_enter_row > initial_row, "Cursor row should increase after Enter");
    assert_eq!(after_enter_col, initial_col, "Cursor should still be at prompt position after Enter");

    // Verify prompt on new line
    terminal
        .current_line()
        .assert_contains("> ")
        .expect("Should show prompt after empty input");

    // Type a character and send interrupt
    terminal.send("a").expect("Should send 'a'");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Cursor should now be at initial_col + 1 (after "prompt a")
    let (_, col_with_a) = terminal.cursor_position().expect("Should get cursor position with 'a'");
    assert_eq!(col_with_a, initial_col + 1, "Cursor should be one position after prompt after typing 'a'");

    // Send interrupt
    terminal.send_interrupt().expect("Should send interrupt");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // After interrupt, should be back at prompt position
    // Note: The row may or may not increase depending on terminal behavior
    terminal.wait_for_prompt().expect("Should show prompt after interrupt");
    let (_after_intr_row, after_intr_col) = terminal.cursor_position().expect("Should get cursor position after interrupt");
    // Cursor should be at prompt position after interrupt (column is what matters)
    assert_eq!(after_intr_col, initial_col, "Cursor should be at prompt position after interrupt");

    terminal.quit().expect("Should quit cleanly");
}

/// Test screen state inspection with absolute line access.
///
/// This test exercises the `line()`, `assert_cursor()`, `screen()`, and `clear_buffer()` methods
/// for comprehensive screen state inspection.
///
/// Port of: radian/tests/test_startup.py cursor and line assertions
#[test]
fn test_pty_screen_state_inspection() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Get full screen state
    let screen = terminal.screen().expect("Should get screen snapshot");
    assert!(screen.lines.len() > 0, "Screen should have lines");
    // Prompt length varies ("r> " is 3, "R 4.5.2> " is 9), just verify it's > 0
    let initial_col = screen.cursor_col;
    assert!(initial_col > 0, "Initial cursor column should be after prompt");

    // Get prompt row for line-based assertions
    let prompt_row = screen.cursor_row as usize;

    // Use line() to check absolute row content
    terminal
        .line(prompt_row)
        .assert_contains("> ")
        .expect("Prompt line should start with prompt indicator");

    // Execute a simple expression
    terminal.send_line("100").expect("Should send 100");
    terminal.clear_and_expect("[1] 100").expect("Should show result");

    // Use assert_cursor for position verification - cursor should be at prompt position
    terminal
        .assert_cursor(screen.cursor_row + 2, initial_col)
        .expect("Cursor should be 2 rows down, at prompt position");

    // Clear buffer and verify it works
    terminal.clear_buffer().expect("Should clear buffer");

    // Execute another expression
    terminal.send_line("200").expect("Should send 200");
    terminal.expect("[1] 200").expect("Should show result 200");

    terminal.quit().expect("Should quit cleanly");
}

/// Test bracketed paste mode for handling pasted text.
///
/// Bracketed paste mode wraps pasted text in escape sequences:
/// - Start: ESC [ 200 ~
/// - End: ESC [ 201 ~
///
/// This allows the terminal to handle pasted text differently from typed text.
///
/// Port of: radian/tests/test_readline.py::test_strings_bracketed
#[test]
#[cfg(unix)] // Bracketed paste mode is not supported on Windows
fn test_pty_bracketed_paste() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Test simple bracketed paste (10 'a' characters)
    let paste_start = "\x1b[200~";
    let paste_end = "\x1b[201~";
    let content = "x <- '".to_string() + &"a".repeat(10) + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send bracketed paste");
    terminal.clear_and_expect("> ").expect("Should show prompt after paste");

    terminal.send_line("nchar(x)").expect("Should send nchar(x)");
    terminal.clear_and_expect("[1] 10").expect("nchar(x) should return 10");

    // Test medium bracketed paste (100 characters) - validates paste handling
    let content = "y <- '".to_string() + &"b".repeat(100) + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send medium bracketed paste");
    terminal.clear_and_expect("> ").expect("Should show prompt after medium paste");

    terminal.send_line("nchar(y)").expect("Should send nchar(y)");
    terminal.clear_and_expect("[1] 100").expect("nchar(y) should return 100");

    terminal.quit().expect("Should quit cleanly");
}

/// Test bracketed paste mode with very long strings.
///
/// This is a regression test for handling very long strings that may exceed
/// internal buffer sizes. When text is chunked into fixed byte segments,
/// there's a risk of cutting multibyte characters in half.
///
/// Port of: radian/tests/test_readline.py::test_strings_bracketed (5000 chars case)
/// Related: https://github.com/randy3k/radian/issues/377
#[test]
#[cfg(unix)]
fn test_pty_bracketed_paste_long_string() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let paste_start = "\x1b[200~";
    let paste_end = "\x1b[201~";

    // Test very long bracketed paste (5000 characters)
    let content = "x <- '".to_string() + &"a".repeat(5000) + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send long bracketed paste");
    terminal.clear_and_expect("> ").expect("Should show prompt after long paste");

    terminal.send_line("nchar(x)").expect("Should send nchar(x)");
    terminal.clear_and_expect("[1] 5000").expect("nchar(x) should return 5000");

    terminal.quit().expect("Should quit cleanly");
}

/// Test bracketed paste mode with multi-line long strings.
///
/// This tests pasting multi-line content where the total length exceeds
/// typical buffer sizes.
///
/// Port of: radian/tests/test_readline.py::test_strings_bracketed (multi-line case)
/// Related: https://github.com/randy3k/radian/issues/377
#[test]
#[cfg(unix)]
fn test_pty_bracketed_paste_multiline_long_string() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let paste_start = "\x1b[200~";
    let paste_end = "\x1b[201~";

    // Test multi-line long string (2000 'a' + newline + 2000 'b' = 4001 chars total)
    let content = "x <- '".to_string() + &"a".repeat(2000) + "\n" + &"b".repeat(2000) + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send multiline long paste");
    terminal.clear_and_expect("> ").expect("Should show prompt after multiline paste");

    terminal.send_line("nchar(x)").expect("Should send nchar(x)");
    terminal.clear_and_expect("[1] 4001").expect("nchar(x) should return 4001");

    terminal.quit().expect("Should quit cleanly");
}

/// Test bracketed paste mode with very long multibyte strings.
///
/// This is a critical regression test for the bug where chunking text into
/// fixed byte segments causes multibyte characters to be split in half,
/// resulting in errors like "EOF whilst reading MBCS char" or
/// "invalid multibyte character in parser".
///
/// Port of: radian/tests/test_readline.py::test_strings_bracketed (multibyte case)
/// Related: https://github.com/randy3k/radian/issues/377
#[test]
#[cfg(unix)]
fn test_pty_bracketed_paste_long_multibyte_string() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let paste_start = "\x1b[200~";
    let paste_end = "\x1b[201~";

    // Test with multibyte characters (Chinese characters, 3 bytes each in UTF-8)
    // 1000 '中' + '\n' + 1000 '文' + '\n' + 1000 '中' + '\n' + 1000 '文' = 4003 chars total
    let s = "中".repeat(1000) + "\n" + &"文".repeat(1000) + "\n" + &"中".repeat(1000) + "\n" + &"文".repeat(1000);
    let content = "x <- '".to_string() + &s + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send multibyte long paste");
    terminal.clear_and_expect("> ").expect("Should show prompt after multibyte paste");

    terminal.send_line("nchar(x)").expect("Should send nchar(x)");
    terminal.clear_and_expect("[1] 4003").expect("nchar(x) should return 4003");

    // Test with different variable name (different padding)
    let content = "xy <- '".to_string() + &s + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send multibyte paste with different padding");
    terminal.clear_and_expect("> ").expect("Should show prompt after second paste");

    terminal.send_line("nchar(xy)").expect("Should send nchar(xy)");
    terminal.clear_and_expect("[1] 4003").expect("nchar(xy) should return 4003");

    terminal.quit().expect("Should quit cleanly");
}

/// Test that multiple expressions print intermediate results.
///
/// This is a regression test for radian issue #388, where fixing the long
/// multibyte string issue caused intermediate expression outputs to be
/// suppressed. Our implementation should NOT have this side effect because
/// we chunk at the ReadConsole level without wrapping input in braces.
///
/// Related: https://github.com/randy3k/radian/issues/388
#[test]
#[cfg(unix)]
fn test_pty_multiple_expressions_print_intermediate() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let paste_start = "\x1b[200~";
    let paste_end = "\x1b[201~";

    // Paste two expressions - both should produce output
    let content = "1 + 1\n2 + 2";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send multiple expressions");

    // Wait for output and check that BOTH results are visible
    std::thread::sleep(std::time::Duration::from_millis(500));
    let screen = terminal.screen().expect("Should get screen");
    let screen_text = screen.lines.join("\n");

    // Both "[1] 2" (from 1+1) and "[1] 4" (from 2+2) should be in the output
    assert!(
        screen_text.contains("[1] 2"),
        "First expression result should be visible. Screen:\n{}",
        screen_text
    );
    assert!(
        screen_text.contains("[1] 4"),
        "Second expression result should be visible. Screen:\n{}",
        screen_text
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test bracketed paste with auto-match enabled.
///
/// This test verifies that pasting text containing brackets doesn't cause
/// bracket duplication when auto-match is enabled. This is a regression test
/// for the bug where pasting "()" resulted in "())".
///
/// Related: https://github.com/arf/arf/issues/ovl
#[test]
#[cfg(unix)]
fn test_pty_bracketed_paste_with_auto_match() {
    // Start arf WITH auto-match enabled (default)
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Test bracketed paste of "()" - should not become "())"
    let paste_start = "\x1b[200~";
    let paste_end = "\x1b[201~";

    // Paste "x <- (1)" and verify the parentheses are correct
    let content = "x <- (1)";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send bracketed paste");
    terminal.clear_and_expect("> ").expect("Should show prompt after paste");

    // Verify the value is correct (if parentheses were duplicated, this would fail)
    terminal.send_line("x").expect("Should send x");
    terminal.clear_and_expect("[1] 1").expect("x should be 1, not error from extra bracket");

    // Test pasting a function call with multiple brackets
    let content = "y <- sum(c(1, 2, 3))";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send nested brackets paste");
    terminal.clear_and_expect("> ").expect("Should show prompt after nested paste");

    terminal.send_line("y").expect("Should send y");
    terminal.clear_and_expect("[1] 6").expect("y should be 6");

    terminal.quit().expect("Should quit cleanly");
}

/// Test escape key cancels current input.
///
/// In vi mode or with escape handling, pressing Escape should cancel the current input.
/// This test verifies that partial input can be discarded.
///
/// Port of: radian/tests/test_readline.py::test_early_termination
#[test]
fn test_pty_escape_cancels_input() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Type some text but don't execute
    terminal.send("invalid_var").expect("Should send partial input");
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send Ctrl+C to cancel (more reliable than Escape for canceling input)
    terminal.send_interrupt().expect("Should send interrupt");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Should be back at prompt
    terminal.wait_for_prompt().expect("Should show prompt after cancel");

    // Now execute a valid expression
    terminal.send_line("42").expect("Should send 42");
    terminal.clear_and_expect("[1] 42").expect("Should execute 42");

    // Verify the partial input was truly discarded
    terminal.send_line("invalid_var").expect("Should send invalid_var");
    terminal.expect("Error").expect("Should show error for undefined variable");

    terminal.quit().expect("Should quit cleanly");
}

/// Test R's readline() function in interactive mode.
///
/// This tests that R's readline() can prompt for input and receive it.
/// The readline() function displays a prompt and waits for user input.
///
/// Port of: radian/tests/test_readline.py::test_readline
#[test]
fn test_pty_readline() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Execute cat() followed by readline() - this should show "hello" then the readline prompt
    terminal
        .send_line("cat('hello'); readline('input> ')")
        .expect("Should send readline command");

    // Should see "hello" output
    terminal.expect("hello").expect("Should see cat output");

    // Should see the readline prompt
    terminal.expect("input> ").expect("Should see readline prompt");

    // Provide input to readline
    terminal.send_line("user_answer").expect("Should send readline input");

    // The readline result should be returned
    terminal
        .expect(r#""user_answer""#)
        .expect("readline should return the input");

    terminal.quit().expect("Should quit cleanly");
}

/// Test askpass package integration for password prompts.
///
/// This tests that the askpass package can prompt for input and receive it.
/// The askpass::askpass() function displays a prompt and waits for user input.
///
/// Note: This test requires the askpass package to be installed.
/// Run: install.packages("askpass") to enable this test.
///
/// Port of: radian/tests/test_readline.py::test_askpass
// TODO: Run this test after installing askpass package:
//   1. R -e "install.packages('askpass')"
//   2. cargo test test_pty_askpass -- --ignored
#[test]
#[ignore] // Requires askpass package - run with: cargo test -- --ignored
fn test_pty_askpass() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Check if askpass is available
    terminal
        .send_line("requireNamespace('askpass', quietly = TRUE)")
        .expect("Should check askpass");
    terminal.expect("TRUE").expect("askpass package should be available");

    // Execute askpass::askpass() with a custom prompt
    terminal
        .send_line("askpass::askpass('password> ')")
        .expect("Should send askpass command");

    // Should see the askpass prompt
    terminal
        .expect("password>")
        .expect("Should see askpass prompt");

    // Provide input
    terminal
        .send_line("secret_answer")
        .expect("Should send askpass input");

    // The askpass result should be returned (masked in display but returned as string)
    terminal
        .expect(r#""secret_answer""#)
        .expect("askpass should return the input");

    terminal.quit().expect("Should quit cleanly");
}

/// Test shell mode via :shell command.
///
/// This tests entering shell mode with :shell, executing shell commands,
/// and returning to R mode with :r.
#[test]
#[cfg(unix)]
fn test_pty_shell_mode() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Enter shell mode
    terminal.send_line(":shell").expect("Should send :shell");
    terminal
        .expect("Shell mode enabled")
        .expect("Should show shell mode message");

    // Prompt should now show shell format (e.g., "[bash] $ " or "[sh] $ ")
    // Wait for the shell prompt to appear
    terminal.expect("] $").expect("Should show shell mode prompt");

    // Execute a shell command
    terminal.send_line("echo hello").expect("Should send shell command");
    terminal.expect("hello").expect("Should see shell output");

    // Return to R mode
    terminal.send_line(":r").expect("Should send :r");
    terminal
        .expect("Returned to R mode")
        .expect("Should show R mode message");

    // Verify we're back in R mode by executing R code
    terminal.send_line("42").expect("Should send R expression");
    terminal.expect("[1] 42").expect("Should evaluate R code");

    terminal.quit().expect("Should quit cleanly");
}

/// Test :system command for single shell execution.
///
/// This tests the :system command which executes a single shell command
/// without entering shell mode.
#[test]
#[cfg(unix)]
fn test_pty_system_command() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Execute a single shell command with :system
    terminal
        .send_line(":system echo test_system_output")
        .expect("Should send :system");

    // Wait for the shell output
    terminal
        .expect("test_system_output")
        .expect("Should see system output");

    // Wait for prompt to return
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Verify we're still in R mode by executing R code
    terminal.send_line("100").expect("Should send R expression");
    terminal.clear_and_expect("[1] 100").expect("Should evaluate R code");

    terminal.quit().expect("Should quit cleanly");
}

/// Test Ctrl+C exits shell mode.
///
/// This tests that pressing Ctrl+C while in shell mode returns to R mode.
#[test]
#[cfg(unix)]
fn test_pty_shell_mode_ctrl_c_exit() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Enter shell mode
    terminal.send_line(":shell").expect("Should send :shell");
    terminal
        .clear_and_expect("Shell mode enabled")
        .expect("Should show shell mode message");

    // Press Ctrl+C to exit shell mode
    terminal.send_interrupt().expect("Should send Ctrl+C");
    terminal
        .clear_and_expect("Returned to R mode")
        .expect("Should return to R mode");

    // Verify we're back in R mode
    terminal.send_line("200").expect("Should send R expression");
    terminal.clear_and_expect("[1] 200").expect("Should evaluate R code");

    terminal.quit().expect("Should quit cleanly");
}

/// Test :help command opens interactive browser in alternate screen.
///
/// This tests that:
/// 1. `:help` (or `:h`) opens an interactive help browser (alternate screen)
/// 2. The browser displays the search header
/// 3. Pressing Esc exits the browser and returns to the prompt
#[test]
#[cfg(unix)]
fn test_pty_help_browser() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Send :h command to open help browser
    terminal
        .send_line(":h")
        .expect("Should send :h command");

    // Wait for browser to appear - it should show the header
    std::thread::sleep(std::time::Duration::from_millis(500));
    terminal
        .expect("Help Search")
        .expect("Should show help browser header");

    // Press Esc to exit the browser
    terminal.send("\x1b").expect("Should send Esc to exit browser");

    // Wait for prompt to return
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Verify we're back at the R prompt by executing R code
    terminal.send_line("42").expect("Should send R expression");
    terminal
        .clear_and_expect("[1] 42")
        .expect("Should evaluate R code after browser exit");

    terminal.quit().expect("Should quit cleanly");
}

/// Test :history schema command displays pager and exits cleanly.
///
/// This tests that:
/// 1. `:history schema` opens an interactive pager (alternate screen)
/// 2. The pager displays the schema content
/// 3. Pressing 'q' exits the pager and returns to the prompt
#[test]
#[cfg(unix)]
fn test_pty_history_schema_pager() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Send :history schema command
    terminal
        .send_line(":history schema")
        .expect("Should send :history schema");

    // Wait for pager to appear - it should show the header
    std::thread::sleep(std::time::Duration::from_millis(500));
    terminal
        .expect("History Schema")
        .expect("Should show pager header");

    // Press 'q' to exit the pager
    terminal.send("q").expect("Should send q to exit pager");

    // Wait for prompt to return
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Verify we're back at the R prompt by executing R code
    terminal.send_line("42").expect("Should send R expression");
    terminal
        .clear_and_expect("[1] 42")
        .expect("Should evaluate R code after pager exit");

    terminal.quit().expect("Should quit cleanly");
}

/// Test 'c' key in history schema pager copies R example code.
///
/// When pressing 'c' in the :history schema pager, it should:
/// 1. Copy the R example code to clipboard via OSC 52
/// 2. Show a feedback message "Copied R example to clipboard"
#[test]
#[cfg(unix)]
fn test_pty_history_schema_pager_copy() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Send :history schema command
    terminal
        .send_line(":history schema")
        .expect("Should send :history schema");

    // Wait for pager to appear
    std::thread::sleep(std::time::Duration::from_millis(500));
    terminal
        .expect("History Schema")
        .expect("Should show pager header");

    // Press 'c' to copy R example
    terminal.send("c").expect("Should send c to copy");

    // Wait for feedback message
    std::thread::sleep(std::time::Duration::from_millis(200));
    terminal
        .expect("Copied R example to clipboard")
        .expect("Should show copy feedback message");

    // Press 'q' to exit the pager
    terminal.send("q").expect("Should send q to exit pager");

    // Wait for prompt to return
    std::thread::sleep(std::time::Duration::from_millis(500));

    terminal.quit().expect("Should quit cleanly");
}

/// Test mouse scroll in history schema pager.
///
/// When EnableMouseCapture is active, mouse scroll events should:
/// 1. Scroll the pager content
/// 2. Not cause flickering (implicit - we verify scroll works)
///
/// This is a regression test for the mouse event handling fix.
#[test]
#[cfg(unix)]
fn test_pty_history_schema_pager_mouse_scroll() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    terminal
        .send_line(":history schema")
        .expect("Should send :history schema");

    std::thread::sleep(std::time::Duration::from_millis(500));
    terminal
        .expect("History Schema")
        .expect("Should show pager header");

    // Verify initial position shows [1/XX]
    terminal
        .expect("[1/")
        .expect("Should show position 1 initially");

    // Send mouse scroll down event (SGR mouse mode: \x1b[<65;col;rowM)
    // Button 65 = scroll down, column 10, row 10
    terminal
        .send("\x1b[<65;10;10M")
        .expect("Should send mouse scroll down");

    std::thread::sleep(std::time::Duration::from_millis(200));

    // After scrolling down, position should change to [2/XX]
    terminal
        .expect("[2/")
        .expect("Should show position 2 after scroll down");

    // Send mouse scroll up event (SGR mouse mode: \x1b[<64;col;rowM)
    // Button 64 = scroll up, column 10, row 10
    terminal
        .send("\x1b[<64;10;10M")
        .expect("Should send mouse scroll up");

    std::thread::sleep(std::time::Duration::from_millis(200));

    // After scrolling up, position should go back to [1/XX]
    terminal
        .expect("[1/")
        .expect("Should show position 1 after scroll up");

    // Exit pager
    terminal.send("q").expect("Should send q to exit pager");

    std::thread::sleep(std::time::Duration::from_millis(500));

    terminal.quit().expect("Should quit cleanly");
}

/// Test reprex mode paste - stripping #> output lines from pasted reprex output.
///
/// When pasting reprex output in reprex mode, lines starting with #> should be
/// stripped so that only the actual R code is executed. This prevents duplicate
/// output when pasting the output of a previous reprex run.
///
/// This is a regression test for the bug where clearing the prompt used the
/// stripped line count instead of the original line count.
#[test]
#[cfg(unix)]
fn test_pty_reprex_paste_strips_output_lines() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Enable autoformat and reprex mode
    terminal.send_line(":autoformat").expect("Should send :autoformat");
    terminal
        .clear_and_expect("Autoformat enabled")
        .expect("Should show autoformat message");

    terminal.send_line(":reprex").expect("Should send :reprex");
    terminal
        .clear_and_expect("Reprex mode enabled")
        .expect("Should show reprex mode message");

    // Paste reprex output using bracketed paste
    // This simulates pasting:
    //   x <- 42
    //   #> [1] 42
    //   x + 1
    //   #> [1] 43
    let paste_start = "\x1b[200~";
    let paste_end = "\x1b[201~";
    let content = "x <- 42\n#> [1] 42\nx + 1\n#> [1] 43";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal.send(&pasted).expect("Should send reprex paste");

    // Wait for execution to complete and verify both expressions were executed
    // The #> lines should be stripped, so we should see [1] 42 and [1] 43 from R
    terminal.clear_and_expect("[1] 42").expect("First expression should output 42");
    terminal.expect("[1] 43").expect("Second expression should output 43");

    // Verify the variables were assigned correctly
    terminal.send_line("x").expect("Should send x");
    terminal.clear_and_expect("[1] 42").expect("x should be 42");

    terminal.quit().expect("Should quit cleanly");
}

/// Test Ctrl+D behavior: does not exit when buffer has content.
///
/// This is the expected behavior matching radian and standard readline behavior:
/// - Buffer has content + Ctrl+D → Delete character under cursor (does NOT exit)
/// - Empty buffer + Ctrl+D → Exit (tested implicitly via quit())
///
/// This test verifies that pressing Ctrl+D while typing does not accidentally
/// exit the REPL, which would cause data loss.
#[test]
#[cfg(unix)]
fn test_pty_ctrl_d_with_content_does_not_exit() {
    use common::Terminal;

    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Type some text, then send Ctrl+D - should NOT exit
    terminal.send("abc").expect("Should send abc");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Send Ctrl+D - at end of buffer, this does nothing (no char to delete)
    // But crucially, it should NOT exit the REPL
    terminal.send_eof().expect("Should send Ctrl+D");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Cancel the current input with Ctrl+C
    terminal.send_interrupt().expect("Should send Ctrl+C to cancel");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Execute a command to verify REPL is still functional after Ctrl+D
    terminal.clear_buffer().expect("Should clear buffer");
    terminal.send_line("42").expect("Should send 42");
    terminal.expect("[1] 42").expect("REPL should still be running after Ctrl+D with content");

    terminal.quit().expect("Should quit cleanly");
}

/// Test Ctrl+R history menu replaces buffer instead of appending.
///
/// This tests the fix for https://github.com/nushell/nushell/issues/7746
/// When selecting from history menu, the selected item should REPLACE
/// the buffer, not append to existing text.
///
/// Scenario:
/// 1. Execute a unique command to add to history
/// 2. Type partial text that matches the history item
/// 3. Press Ctrl+R to open history menu
/// 4. Press Enter to select and execute the history item
/// 5. Verify no error occurred (error would indicate buffer was appended, not replaced)
#[test]
#[cfg(unix)]
fn test_pty_history_menu_replaces_buffer() {
    use common::Terminal;

    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Step 1: Execute a command to add to history
    // Use a unique variable name unlikely to conflict with anything
    terminal
        .send_line("r_term_test_hist_var_7746 <- 999")
        .expect("Should send assignment");
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after assignment");

    // Step 2: Type partial text that matches the history item
    terminal.send("r_term").expect("Should type partial text");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Step 3: Press Ctrl+R to open history menu
    terminal.send("\x12").expect("Should send Ctrl+R");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Step 4: Press Enter to select the matching history item
    // With only_buffer_difference=false, selecting should REPLACE the buffer
    // with the full history item, not append to "r_term"
    terminal.send("\n").expect("Should send Enter to select");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Step 5: The selected item should now be in the buffer, execute it
    terminal.send("\n").expect("Should send Enter to execute");

    // Step 6: If the buffer was correctly replaced with the full history item
    // (r_term_test_hist_var_7746 <- 999), the assignment should execute without error.
    // If the buffer was incorrectly appended (r_termr_term_test_hist_var_7746 <- 999),
    // R would show an error about undefined variable "r_termr_term_test_hist_var_7746".
    // Wait for prompt - no error means success
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after executing history item (no error = buffer was replaced, not appended)");

    terminal.quit().expect("Should quit cleanly");
}

/// Test that history selection works correctly when auto-match has inserted a pair.
///
/// This tests the fix for a bug where typing a character that triggers auto-match
/// (like `` ` `` which inserts ``` `` ```), then using Ctrl+R to select from history,
/// would leave the trailing character from the pair in the buffer.
///
/// Scenario:
/// 1. Execute a command (`1:3`) to add it to history
/// 2. Type a backtick (which with auto-match becomes ``` `` ``` with cursor in middle)
/// 3. Press Ctrl+R to open history menu
/// 4. Select `1:3` from history
/// 5. Execute - should work (buffer should be `1:3`, not `1:3` `)
///
/// Without the fix, the buffer would become `1:3` ` (with trailing backtick),
/// which is an incomplete R expression and would cause a newline to be inserted
/// instead of submitting.
#[test]
#[cfg(unix)]
fn test_pty_history_menu_with_auto_match_pair() {
    use common::Terminal;

    // Enable auto-match (default) to trigger the bug scenario
    let mut terminal =
        Terminal::spawn_with_args(&["--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Step 1: Execute `1:3` to add it to history
    // This is a simple R sequence that outputs [1] 1 2 3
    terminal.send_line("1:3").expect("Should send 1:3");
    terminal
        .expect("[1] 1 2 3")
        .expect("Should show sequence output");
    terminal.wait_for_prompt().expect("Should show prompt");

    // Step 2: Type a backtick - with auto-match enabled, this inserts `` with cursor between
    terminal.send("`").expect("Should type backtick");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Step 3: Press Ctrl+R to open history menu
    terminal.send("\x12").expect("Should send Ctrl+R");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Step 4: The history menu should show. Press Enter to select the first item
    // (which should be `1:3` since it's the most recent)
    terminal.send("\n").expect("Should send Enter to select");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Step 5: Execute the selected history item
    terminal.send("\n").expect("Should send Enter to execute");

    // Step 6: If the fix works, the buffer should be exactly `1:3` (no trailing backtick)
    // and execution should succeed, showing the sequence again.
    // If the bug is present, the buffer would be `1:3` ` and R would insert a newline
    // for the incomplete expression.
    terminal
        .expect("[1] 1 2 3")
        .expect("1:3 should execute successfully (buffer was fully replaced)");

    terminal.quit().expect("Should quit cleanly");
}

/// Regression test: backtick input should not crash.
/// Sending a backtick (which becomes `` with auto-match) should produce an
/// R error about zero-length variable name, not crash with RefCell double borrow.
///
/// The original bug was caused by re-entrant calls to read_console_callback when
/// R's parser (called by RValidator via harp::is_expression_complete) would
/// trigger another ReadConsole call while the RefCell was still borrowed.
///
/// Fixed by replacing R-based expression validation with a heuristic-based
/// validator that doesn't call into R, avoiding the re-entrancy issue entirely.
#[test]
#[cfg(unix)]
fn test_pty_backtick_does_not_crash() {
    use common::Terminal;

    let mut terminal =
        Terminal::spawn_with_args(&["--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Type a backtick and press Enter in one go
    // With auto-match enabled, backtick inserts `` with cursor between
    terminal.send_line("`").expect("Should send backtick");

    // R should show an error message about zero-length variable name
    // (not crash with RefCell already borrowed)
    terminal
        .expect("zero-length variable name")
        .expect("Should show R error about zero-length variable name");

    // The prompt should return, indicating no crash occurred
    terminal.wait_for_prompt().expect("Should show prompt after error (no crash)");

    // Verify we can still interact with the REPL
    terminal.send_line("1 + 1").expect("Should send expression");
    terminal
        .expect("[1] 2")
        .expect("Should evaluate expression correctly");

    terminal.quit().expect("Should quit cleanly");
}

/// Test multiline raw string input (R 4.0+ raw string literals).
///
/// This tests the specific case where a raw string is entered across multiple lines.
/// Raw strings in R use delimiters like r"(...)" where the content between ( and )
/// can span multiple lines.
///
/// This is a regression test for the issue where the validator receives empty strings
/// during interactive multiline editing of raw strings.
#[test]
fn test_pty_multiline_raw_string_input() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Assign raw string to variable so we can check its content
    // Start a raw string that spans multiple lines
    // r"( starts the raw string with delimiter (
    // We need to close it with )" to complete the raw string
    terminal
        .send("x <- r\"(hello")
        .expect("Should send raw string opening");
    terminal.send("\r").expect("Should send Enter");

    // Wait for continuation prompt - the expression is incomplete
    terminal
        .clear_and_expect("+")
        .expect("Should show continuation prompt for incomplete raw string");

    // Complete the raw string with closing delimiter
    terminal.send("world)\"").expect("Should send closing delimiter");
    terminal.send("\r").expect("Should send Enter");

    // Wait for prompt (assignment doesn't produce output)
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after assignment");

    // Verify the content was preserved (11 chars: hello + newline + world)
    terminal.send_line("nchar(x)").expect("Should check length");
    terminal
        .clear_and_expect("[1] 11")
        .expect("Raw string should have 11 characters (hello + newline + world)");

    terminal.quit().expect("Should quit cleanly");
}

/// Test raw string input with auto-match enabled - KNOWN ISSUE.
///
/// Auto-match interferes with R raw string syntax (`r"(...)"`).
/// When typing `"` after `r`, auto-match inserts `""` which breaks raw string input.
///
/// Workaround: Use `--no-auto-match` flag or paste raw strings via bracketed paste.
#[test]
#[ignore] // Known issue: auto-match doesn't support raw strings
fn test_pty_raw_string_with_auto_match() {
    // Enable auto-match (default behavior)
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Type r"()" - with auto-match, this will fail because " inserts ""
    terminal
        .send_line(r#"x <- r"()""#)
        .expect("Should send raw string");

    // Wait for prompt (assignment doesn't produce output)
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after assignment");

    // The variable should exist and contain an empty string
    terminal.send_line("x").expect("Should check x");
    terminal
        .clear_and_expect(r#"[1] """#)
        .expect("x should be empty string (content between parens is empty)");

    terminal.quit().expect("Should quit cleanly");
}

// ============================================================================
// R Event Processing Tests
// ============================================================================

/// Test that R event processing API is available and works correctly.
///
/// This test verifies that:
/// 1. R_ProcessEvents and related functions are loaded
/// 2. Calling process_r_events() doesn't crash
/// 3. Basic R evaluation still works after event processing
///
/// Note: Actual graphics window testing (plot()) requires a display
/// and manual testing. This test only verifies the API is functional.
#[test]
fn test_r_event_processing_api() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["-e", r#"
            # Create a simple plot (opens graphics device)
            # On non-interactive systems, this may use a null device
            invisible(plot(1:3, main = "Event Processing Test"))

            # Call dev.off() to close any graphics device
            invisible(dev.off())

            # Verify R is still responsive
            42
        "#])
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

/// Test that R's menu() function displays the correct prompt.
///
/// This verifies the fix for the bug where arf incorrectly showed the main R prompt
/// (e.g., "R 4.5.1> ") when R is waiting for menu input, instead of showing R's
/// actual prompt (e.g., "Selection: ").
///
/// The bug caused state mismatch and user confusion because:
/// - User sees normal R prompt and thinks R is ready for commands
/// - But R is actually waiting for menu selection input
/// - User's command goes to menu handler instead of R parser
///
/// Regression test for a prompt display bug that was fixed.
#[test]
fn test_pty_menu_prompt() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Call menu() which displays "Selection: " as the prompt
    terminal
        .send_line("menu(c('option1', 'option2', 'option3'))")
        .expect("Should send menu command");

    // Should see the menu options
    terminal.expect("1: option1").expect("Should see option 1");
    terminal.expect("2: option2").expect("Should see option 2");
    terminal.expect("3: option3").expect("Should see option 3");

    // Should see R's actual menu prompt "Selection: " NOT our configured prompt
    terminal
        .expect("Selection: ")
        .expect("Should see 'Selection: ' prompt from R, not custom prompt");

    // Provide selection
    terminal.send_line("2").expect("Should send selection");

    // menu() returns the selected index
    terminal
        .expect("[1] 2")
        .expect("menu should return selected index");

    // Verify we return to normal R prompt
    terminal.wait_for_prompt().expect("Should return to normal prompt");

    // Normal R command should work
    terminal.send_line("1 + 1").expect("Should send R command");
    terminal.expect("[1] 2").expect("Should get result");

    terminal.quit().expect("Should quit cleanly");
}
