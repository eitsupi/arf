//! Basic PTY integration tests for arf.
//!
//! These tests cover startup, Ctrl+C, basic expression evaluation, multiline input,
//! error handling, history exit status tracking, rlang error detection, and
//! sponge-like history forget.
//!
//! All tests are Unix-only because crossterm's `cursor::position()` uses WinAPI
//! on Windows, which doesn't work correctly inside ConPTY.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Test that arf starts up correctly and shows the prompt.
///
/// This test uses a custom PTY proxy that responds to cursor position queries
/// (CSI 6n) from reedline, enabling proper interactive testing.
#[test]
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
fn test_pty_variable_assignment() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Assign a string variable
    terminal
        .send_line("x <- 'hello'")
        .expect("Should send assignment");

    // Clear buffer and wait for fresh prompt (no output for assignment)
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after assignment");

    // Check the variable length
    terminal
        .send_line("nchar(x)")
        .expect("Should send nchar(x)");

    // Clear buffer and wait for result
    terminal
        .clear_and_expect("[1] 5")
        .expect("nchar(x) should return 5");

    terminal.quit().expect("Should quit cleanly");
}

/// Test cat() output in PTY mode.
///
/// Tests that R's cat() function works correctly in interactive mode.
/// Note: readline() requires special handling that may not be fully supported yet.
#[test]
#[cfg(unix)]
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
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after cat");

    terminal.quit().expect("Should quit cleanly");
}

/// Test multiline expression handling.
#[test]
#[cfg(unix)]
fn test_pty_multiline_input() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Start a function definition (incomplete expression)
    terminal
        .send_line("f <- function(x) {")
        .expect("Should send first line");

    // Wait for continuation prompt
    terminal
        .clear_and_expect("+")
        .expect("Should show continuation prompt");

    // Complete the function
    terminal
        .send_line("  x + 1")
        .expect("Should send second line");
    terminal
        .clear_and_expect("+")
        .expect("Should show continuation prompt again");

    terminal.send_line("}").expect("Should send closing brace");

    // Wait for normal prompt to return
    terminal
        .clear_and_expect("> ")
        .expect("Should show normal prompt");

    // Test the function
    terminal.send_line("f(10)").expect("Should call function");
    terminal
        .clear_and_expect("[1] 11")
        .expect("f(10) should return 11");

    terminal.quit().expect("Should quit cleanly");
}

/// Test multiline string input (string literal spanning multiple lines).
/// This tests the specific case where a string with embedded newline is entered
/// across multiple lines, which requires proper handling by the validator.
#[test]
#[cfg(unix)]
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
    terminal
        .send("end\"")
        .expect("Should send closing text and quote");
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
#[cfg(unix)]
fn test_pty_error_handling() {
    // Use --no-auto-match to avoid bracket insertion interfering with the test
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Trigger an error
    terminal
        .send_line("stop('Test error')")
        .expect("Should send stop()");

    // Should see error message
    terminal.expect("Error").expect("Should see error output");

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
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--history-dir", &history_dir])
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
///
/// Requires dplyr to be installed.
#[test]
#[cfg(unix)]
fn test_pty_rlang_error_detection() {
    if !common::has_dplyr() {
        eprintln!("Skipping test: dplyr not available");
        return;
    }
    use reedline::{History, SearchDirection, SearchQuery, SqliteBackedHistory};

    // Create a temporary directory for history
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let history_dir = temp_dir.path().to_string_lossy().to_string();

    // Start arf with custom history directory
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match", "--history-dir", &history_dir])
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
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after error");

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
    let success_commands: Vec<_> = entries.iter().filter(|e| e.command_line == "1").collect();
    assert_eq!(
        success_commands.len(),
        1,
        "Successful command should be present"
    );
}
