//! Advanced PTY integration tests for arf.
//!
//! These tests cover reprex paste stripping, Ctrl+D behavior, history menu
//! buffer replacement, auto-match with history selection, backtick crash
//! regression, raw string input, R event processing API, menu prompt display,
//! and vi mode indicator.
//!
//! Most tests are Unix-only because crossterm's `cursor::position()` uses WinAPI
//! on Windows, which doesn't work correctly inside ConPTY.

mod common;

#[cfg(unix)]
use common::Terminal;

use std::process::Command;

/// Test reprex mode paste - stripping #> output lines from pasted reprex output.
///
/// When pasting reprex output in reprex mode, lines starting with #> should be
/// stripped so that only the actual R code is executed. This prevents duplicate
/// output when pasting the output of a previous reprex run.
///
/// This is a regression test for the bug where clearing the prompt used the
/// stripped line count instead of the original line count.
///
/// Requires Air CLI for reprex format functionality.
#[test]
#[cfg(unix)]
fn test_pty_reprex_paste_strips_output_lines() {
    if !common::has_air_cli() {
        eprintln!("Skipping test: Air CLI not available");
        return;
    }

    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    terminal
        .send_line(":reprex on")
        .expect("Should send :reprex on");
    terminal
        .clear_and_expect("Reprex: on")
        .expect("Should show reprex mode message");
    terminal
        .send_line(":reprex format")
        .expect("Should send :reprex format");
    terminal
        .clear_and_expect("Reprex: format")
        .expect("Should show reprex format message");

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
    terminal
        .clear_and_expect("[1] 42")
        .expect("First expression should output 42");
    terminal
        .expect("[1] 43")
        .expect("Second expression should output 43");

    // Verify the variables were assigned correctly
    terminal.send_line("x").expect("Should send x");
    terminal.clear_and_expect("[1] 42").expect("x should be 42");

    terminal.quit().expect("Should quit cleanly");
}

/// Formatter failures must be treated as failed commands even though R never
/// evaluates the input.  This covers prompt status/duration, history metadata,
/// and the sponge queue on the same path.
#[test]
#[cfg(unix)]
fn test_pty_formatter_failure_updates_lifecycle_without_evaluation() {
    use reedline::{History, SearchDirection, SearchQuery, SqliteBackedHistory};
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("create temporary test directory");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir(&bin_dir).expect("create fake formatter directory");
    let arity = bin_dir.join("arity");
    std::fs::write(
        &arity,
        r##"#!/bin/sh
case "$1" in
  --version) echo 'arity 0.19.1'; exit 0 ;;
  format)
    input=$(cat)
    if [ "$input" = 42 ]; then
      echo 'synthetic formatter failure' >&2
      exit 17
    fi
    printf '%s' "$input"
    exit 0
    ;;
  *) exit 18 ;;
esac
"##,
    )
    .expect("write fake formatter");
    let mut permissions = std::fs::metadata(&arity)
        .expect("stat fake formatter")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&arity, permissions).expect("make fake formatter executable");

    let config_path = temp_dir.path().join("arf.toml");
    std::fs::write(
        &config_path,
        r#"
[startup]
reprex = "format"

[reprex]
formatter = "arity"

[prompt]
format = "{status}{duration}R> "

[prompt.status.symbol]
error = "ERR "

[experimental.prompt_duration]
threshold_ms = 0

[experimental.prompt_spinner]
frames = ""

[experimental.history_forget]
enabled = true
delay = 1
on_exit_only = false
"#,
    )
    .expect("write test config");

    let history_dir = temp_dir.path().join("history");
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").expect("PATH should be set")
    );
    let config_path = config_path.to_string_lossy().into_owned();
    let history_dir = history_dir.to_string_lossy().into_owned();
    let mut terminal = Terminal::spawn_with_args_and_env(
        &["--config", &config_path, "--history-dir", &history_dir],
        &[("PATH", &path)],
    )
    .expect("spawn arf with fake formatter");

    terminal.wait_for_prompt().expect("show initial prompt");

    // Establish a visible previous duration, then trigger a formatter error.
    terminal
        .clear_buffer()
        .expect("clear initial prompt output");
    terminal.send_line("1").expect("send successful command");
    terminal
        .expect("[1] 1")
        .expect("successful command should run");
    terminal
        .expect("R> ")
        .expect("successful command should reach the next prompt");
    terminal
        .current_line()
        .assert_contains("ms")
        .expect("successful command should leave a visible duration");

    terminal
        .clear_buffer()
        .expect("clear successful command output");
    terminal
        .send_line("42")
        .expect("send command that formatter rejects");
    terminal
        .expect("Arity formatting failed: synthetic formatter failure")
        .expect("formatter error should be shown");
    terminal
        .expect("R> ")
        .expect("formatter failure should reach the failed prompt");

    terminal
        .current_line()
        .assert_contains("ERR ")
        .expect("formatter failure should mark the next prompt failed");
    let screen = terminal
        .screen()
        .expect("read screen after formatter failure");
    let current_line = screen
        .lines
        .get(screen.cursor_row as usize)
        .expect("cursor row should exist");
    assert!(
        !current_line.contains("ms"),
        "formatter failure should clear the previous duration: {current_line:?}"
    );
    assert!(
        !terminal
            .get_output()
            .expect("read terminal output")
            .contains("[1] 42"),
        "formatter-rejected code must not be evaluated"
    );

    let history_path = temp_dir.path().join("history").join("r.db");
    {
        let history = SqliteBackedHistory::with_file(history_path.clone(), None, None)
            .expect("open history while session is running");
        let entries = history
            .search(SearchQuery::everything(SearchDirection::Forward, None))
            .expect("search history after formatter failure");
        let failed_entry = entries
            .iter()
            .find(|entry| entry.command_line == "42")
            .expect("formatter-rejected command should be in history");
        assert_eq!(failed_entry.exit_status, Some(1));
    }

    // A following successful command advances the sponge queue and removes
    // the failed formatter command after the configured delay.
    terminal
        .clear_buffer()
        .expect("clear failed command output");
    terminal.send_line("1").expect("send follow-up command");
    terminal
        .expect("[1] 1")
        .expect("follow-up command should run");
    terminal
        .expect("R> ")
        .expect("follow-up command should reach the next prompt");
    let history = SqliteBackedHistory::with_file(history_path, None, None)
        .expect("reopen history after sponge update");
    let entries = history
        .search(SearchQuery::everything(SearchDirection::Forward, None))
        .expect("search history after sponge update");
    assert!(
        !entries.iter().any(|entry| entry.command_line == "42"),
        "failed formatter command should be removed by history forget"
    );

    terminal.quit().expect("quit cleanly");
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
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Type some text, then send Ctrl+D - should NOT exit
    terminal.send("abc").expect("Should send abc");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Send Ctrl+D - at end of buffer, this does nothing (no char to delete)
    // But crucially, it should NOT exit the REPL
    terminal.send_eof().expect("Should send Ctrl+D");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Cancel the current input with Ctrl+C
    terminal
        .send_interrupt()
        .expect("Should send Ctrl+C to cancel");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Execute a command to verify REPL is still functional after Ctrl+D
    terminal.clear_buffer().expect("Should clear buffer");
    terminal.send_line("42").expect("Should send 42");
    terminal
        .expect("[1] 42")
        .expect("REPL should still be running after Ctrl+D with content");

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
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

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

/// Test that history selection replaces the whole auto-matched pair buffer.
///
/// Scenario:
/// 1. Execute `length(c())` to add it to history
/// 2. Type `c(`, which auto-match expands to `c()` with the cursor before `)`
/// 3. Press Ctrl+R and select the `length(c())` history entry
/// 4. Execute - should run `length(c())`, not the pre-menu `c()` or a stale-suffix `length(c()))`
#[test]
#[cfg(unix)]
fn test_pty_history_menu_with_auto_match_paren_pair() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    terminal
        .send_line("length(c())")
        .expect("Should send length(c())");
    terminal
        .expect("[1] 0")
        .expect("length(c()) should output 0");
    terminal.wait_for_prompt().expect("Should show prompt");

    terminal.send("c(").expect("Should type c(");
    std::thread::sleep(std::time::Duration::from_millis(200));
    terminal
        .current_line()
        .assert_contains("c()")
        .expect("Auto-match should insert the closing paren before history selection");

    terminal.send("\x12").expect("Should send Ctrl+R");
    std::thread::sleep(std::time::Duration::from_millis(500));

    terminal.send("\n").expect("Should select history item");
    std::thread::sleep(std::time::Duration::from_millis(300));

    terminal
        .clear_buffer()
        .expect("Should clear setup output before executing selection");
    terminal
        .send("\n")
        .expect("Should execute selected history item");
    terminal
        .expect("[1] 0")
        .expect("length(c()) should execute without a trailing auto-matched paren");

    terminal.quit().expect("Should quit cleanly");
}

/// Test that history search treats its query as literal input.
///
/// Scenario:
/// 1. Execute `length(c())` to add it to history
/// 2. Press Ctrl+R to open history search
/// 3. Type `c(` in the search input; reverse-history search is not an edit buffer
///    and therefore does not auto-pair the query
/// 4. Select the `length(c())` history entry
/// 5. Execute - should run `length(c())`, not the search buffer `c()` or a stale-suffix `length(c()))`
#[test]
#[cfg(unix)]
fn test_pty_history_menu_search_mode_literal_paren_query() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-completion"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    terminal
        .send_line("length(c())")
        .expect("Should send length(c())");
    terminal
        .expect("[1] 0")
        .expect("length(c()) should output 0");
    terminal.wait_for_prompt().expect("Should show prompt");

    terminal.send("\x12").expect("Should send Ctrl+R");
    std::thread::sleep(std::time::Duration::from_millis(500));

    terminal
        .send("c(")
        .expect("Should type c( in history search mode");
    std::thread::sleep(std::time::Duration::from_millis(300));
    terminal
        .current_line()
        .assert_contains("c(")
        .expect("History search should preserve the literal query");

    terminal.send("\n").expect("Should select history item");
    std::thread::sleep(std::time::Duration::from_millis(300));

    terminal
        .clear_buffer()
        .expect("Should clear setup output before executing selection");
    terminal
        .send("\n")
        .expect("Should execute selected history item");
    terminal
        .expect("[1] 0")
        .expect("length(c()) should execute without a trailing auto-matched paren");

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
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after error (no crash)");

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
#[cfg(unix)]
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
    terminal
        .send("world)\"")
        .expect("Should send closing delimiter");
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

/// Test raw string input with auto-match enabled.
///
/// The highlighter suppresses the initial quote pair after an R raw-string
/// prefix, allowing the following brackets and closing quote to be typed
/// normally.
#[test]
#[cfg(unix)]
fn test_pty_raw_string_with_auto_match() {
    // Enable auto-match (default behavior)
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Type a dashed raw string containing an internal quote; the raw-string
    // prefix prevents the opening quote from inserting a second quote, and
    // the raw scanner keeps the body quote from being paired as well.
    terminal
        .send_line(r#"x <- r"---(hello "world")---""#)
        .expect("Should send raw string with an internal quote");

    // Wait for prompt (assignment doesn't produce output)
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after assignment");

    // The variable should preserve the raw body and its internal quote.
    terminal
        .send_line("nchar(x)")
        .expect("Should check raw string length");
    terminal
        .clear_and_expect("[1] 13")
        .expect("x should preserve the raw body and its internal quote");

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
#[cfg(unix)]
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
    terminal
        .wait_for_prompt()
        .expect("Should return to normal prompt");

    // Normal R command should work
    terminal.send_line("1 + 1").expect("Should send R command");
    terminal.expect("[1] 2").expect("Should get result");

    terminal.quit().expect("Should quit cleanly");
}

/// A custom R continuation option must still be classified as a continuation
/// by the console. The fake formatter makes the low-level R ReadConsole path
/// observable despite reedline handling ordinary incomplete input itself.
#[test]
#[cfg(unix)]
fn test_pty_custom_continuation_prompt_round_trip() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("create temporary test directory");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir(&bin_dir).expect("create fake formatter directory");
    let arity = bin_dir.join("arity");
    std::fs::write(
        &arity,
        r##"#!/bin/sh
case "$1" in
  --version) echo 'arity 0.19.1'; exit 0 ;;
  format)
    input=$(cat)
    if [ "$input" = 42 ]; then
      printf '%s' '1 +'
    else
      printf '%s' "$input"
    fi
    exit 0
    ;;
  *) exit 18 ;;
esac
"##,
    )
    .expect("write fake formatter");
    let mut permissions = std::fs::metadata(&arity)
        .expect("stat fake formatter")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&arity, permissions).expect("make fake formatter executable");

    let config_path = temp_dir.path().join("arf.toml");
    std::fs::write(
        &config_path,
        r#"
[startup]
reprex = "format"

[reprex]
formatter = "arity"

[prompt]
format = "MAIN> "
continuation = "CONT> "
"#,
    )
    .expect("write test config");

    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").expect("PATH should be set")
    );
    let config_path = config_path.to_string_lossy().into_owned();
    let mut terminal = Terminal::spawn_with_args_and_env(
        &["--no-auto-match", "--config", &config_path],
        &[("PATH", &path)],
    )
    .expect("spawn arf with fake formatter");

    terminal.expect("MAIN> ").expect("Should show main prompt");

    terminal
        .clear_buffer()
        .expect("Should clear initial prompt output");
    terminal
        .send_line("options(continue = '... ')")
        .expect("Should configure continuation prompt");
    terminal
        .expect("MAIN> ")
        .expect("Should return to the normal prompt after options()");

    terminal
        .clear_buffer()
        .expect("Should clear options output before incomplete expression");
    terminal
        .send_line("42")
        .expect("Should send formatter sentinel");
    terminal
        .expect("CONT> ")
        .expect("Should display the configured arf continuation prompt");

    terminal
        .clear_buffer()
        .expect("Should clear continuation prompt output");
    terminal
        .send_line("2")
        .expect("Should complete the expression");
    terminal
        .expect("[1] 3")
        .expect("Should evaluate the expression");
    terminal
        .expect("MAIN> ")
        .expect("Should return to the normal prompt after completion");

    terminal
        .send_line("1 + 1")
        .expect("Should send a subsequent top-level command");
    terminal
        .expect("[1] 2")
        .expect("Subsequent top-level command should execute");

    terminal.quit().expect("Should quit cleanly");
}

/// An identical R main and continuation prompt is ambiguous to arf. Warn once
/// when the session enters that state, suppress repeated warnings, and re-arm
/// the warning after the options become distinct again.
#[test]
#[cfg(unix)]
fn test_pty_identical_r_prompt_options_warn_once_per_transition() {
    const WARNING: &str = "# [arf] Warning: options(\"prompt\") and options(\"continue\") are identical; arf cannot distinguish top-level and continuation prompts.";

    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");
    terminal.wait_for_prompt().expect("Should show prompt");

    terminal.clear_buffer().expect("clear initial output");
    terminal
        .send_line("options(prompt = '> ', continue = '> ')")
        .expect("set identical prompt options");
    terminal.expect(WARNING).expect("warn on first ambiguity");
    terminal
        .expect("+  ")
        .expect("show the continuation prompt while options are ambiguous");
    let output = terminal.get_output().expect("read terminal output");
    assert_eq!(output.matches(WARNING).count(), 1);

    terminal.clear_buffer().expect("clear first warning output");
    terminal.send_line("1 + 1").expect("send command");
    terminal.expect("[1] 2").expect("evaluate command normally");
    terminal
        .expect("+  ")
        .expect("retain continuation classification while ambiguity persists");
    let output = terminal
        .get_output()
        .expect("read output while ambiguity persists");
    assert_eq!(output.matches(WARNING).count(), 0);

    terminal
        .clear_buffer()
        .expect("clear persistent ambiguity output");
    terminal
        .send_line("options(prompt = '> ', continue = '+ ')")
        .expect("restore distinct prompt options");
    terminal
        .wait_for_prompt()
        .expect("return to the normal prompt after restoring options");
    let output = terminal.get_output().expect("read distinct options output");
    assert_eq!(output.matches(WARNING).count(), 0);

    terminal
        .clear_buffer()
        .expect("clear distinct options output");
    terminal
        .send_line("options(prompt = '> ', continue = '> ')")
        .expect("re-enter identical prompt options");
    terminal
        .expect(WARNING)
        .expect("warn again after the ambiguity is reintroduced");
    terminal
        .expect("+  ")
        .expect("show the continuation prompt after second warning");
    let output = terminal.get_output().expect("read second warning output");
    assert_eq!(output.matches(WARNING).count(), 1);

    terminal.quit().expect("Should quit cleanly");
}

/// Test vi mode indicator is displayed correctly at the end of the prompt.
///
/// The vi mode indicator is shown via `render_prompt_indicator()` which ensures
/// it's always synchronized with the actual editing mode (unlike a placeholder
/// approach which would be 1 render cycle behind).
#[test]
#[cfg(unix)]
fn test_pty_vi_mode_indicator() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Create a config file with vi mode and vi symbol configuration
    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    writeln!(
        config_file,
        r#"
[editor]
mode = "vi"

[prompt]
format = "r> "

[prompt.vi]
symbol = {{ insert = "[I]", normal = "[N]" }}
"#
    )
    .expect("Failed to write config file");

    let mut terminal = Terminal::spawn_with_args(&[
        "--config",
        config_file.path().to_str().unwrap(),
        "--no-auto-match",
        "--no-completion",
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Check initial prompt shows insert mode indicator (at end of prompt line)
    let screen = terminal.screen().expect("Should get screen");
    terminal.dump_screen().ok();

    // Find the prompt line - should contain "r> " followed by "[I]"
    let prompt_line = screen.lines.iter().find(|l| l.contains("r> ")).cloned();
    assert!(prompt_line.is_some(), "Should find prompt line with 'r> '");
    let prompt_line = prompt_line.unwrap();
    assert!(
        prompt_line.contains("[I]"),
        "Initial prompt should show [I] for insert mode (at end of prompt), got: {}",
        prompt_line
    );

    // Press Escape to switch to normal mode
    terminal.send("\x1b").expect("Should send Escape");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Check prompt after Escape - should immediately show [N]
    let screen = terminal.screen().expect("Should get screen after Escape");
    eprintln!("=== After Escape ===");
    terminal.dump_screen().ok();

    let prompt_line_after_esc = screen.lines.iter().find(|l| l.contains("r> ")).cloned();
    assert!(
        prompt_line_after_esc
            .as_ref()
            .is_some_and(|l| l.contains("[N]")),
        "After Escape, prompt should show [N] for normal mode, got: {:?}",
        prompt_line_after_esc
    );

    // Press 'i' to go back to insert mode
    terminal.send("i").expect("Should send i");
    std::thread::sleep(std::time::Duration::from_millis(300));

    let screen = terminal.screen().expect("Should get screen after i");
    eprintln!("=== After pressing 'i' (back to insert) ===");
    terminal.dump_screen().ok();

    let prompt_line_after_i = screen.lines.iter().find(|l| l.contains("r> ")).cloned();
    assert!(
        prompt_line_after_i
            .as_ref()
            .is_some_and(|l| l.contains("[I]")),
        "After 'i', prompt should show [I] for insert mode, got: {:?}",
        prompt_line_after_i
    );

    terminal.quit().expect("Should quit cleanly");
}
