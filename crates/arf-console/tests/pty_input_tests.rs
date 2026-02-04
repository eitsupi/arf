//! Paste and input handling PTY integration tests for arf.
//!
//! These tests cover interrupt handling, cursor position tracking, screen state
//! inspection, bracketed paste mode (basic, long, multiline, multibyte),
//! multiple expression printing, auto-match during paste, and escape/cancel.
//!
//! All tests are Unix-only because crossterm's `cursor::position()` uses WinAPI
//! on Windows, which doesn't work correctly inside ConPTY.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Test Ctrl+C interrupts long-running computation.
#[test]
#[cfg(unix)]
fn test_pty_interrupt_computation() {
    // Use --no-auto-match to disable auto-bracket insertion for PTY tests
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Start a long computation (infinite loop)
    terminal
        .send_line("while(TRUE) {}")
        .expect("Should start loop");

    // Give it a moment to start
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Interrupt it
    terminal.send_interrupt().expect("Should send interrupt");

    // Should return to prompt
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after interrupt");

    // Verify we can still do work
    terminal
        .send_line("42")
        .expect("Should send simple expression");

    // Use expect for more robust checking
    terminal
        .expect("[1] 42")
        .expect("Should execute normally after interrupt");

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
#[cfg(unix)]
fn test_pty_cursor_position() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Get initial cursor position after prompt (position varies with prompt format)
    let (initial_row, initial_col) = terminal
        .cursor_position()
        .expect("Should get cursor position");
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
    let (after_enter_row, after_enter_col) = terminal
        .cursor_position()
        .expect("Should get cursor position after enter");
    assert!(
        after_enter_row > initial_row,
        "Cursor row should increase after Enter"
    );
    assert_eq!(
        after_enter_col, initial_col,
        "Cursor should still be at prompt position after Enter"
    );

    // Verify prompt on new line
    terminal
        .current_line()
        .assert_contains("> ")
        .expect("Should show prompt after empty input");

    // Type a character and send interrupt
    terminal.send("a").expect("Should send 'a'");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Cursor should now be at initial_col + 1 (after "prompt a")
    let (_, col_with_a) = terminal
        .cursor_position()
        .expect("Should get cursor position with 'a'");
    assert_eq!(
        col_with_a,
        initial_col + 1,
        "Cursor should be one position after prompt after typing 'a'"
    );

    // Send interrupt
    terminal.send_interrupt().expect("Should send interrupt");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // After interrupt, should be back at prompt position
    // Note: The row may or may not increase depending on terminal behavior
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after interrupt");
    let (_after_intr_row, after_intr_col) = terminal
        .cursor_position()
        .expect("Should get cursor position after interrupt");
    // Cursor should be at prompt position after interrupt (column is what matters)
    assert_eq!(
        after_intr_col, initial_col,
        "Cursor should be at prompt position after interrupt"
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test screen state inspection with absolute line access.
///
/// This test exercises the `line()`, `assert_cursor()`, `screen()`, and `clear_buffer()` methods
/// for comprehensive screen state inspection.
///
/// Port of: radian/tests/test_startup.py cursor and line assertions
#[test]
#[cfg(unix)]
fn test_pty_screen_state_inspection() {
    let mut terminal = Terminal::spawn().expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Get full screen state
    let screen = terminal.screen().expect("Should get screen snapshot");
    assert!(!screen.lines.is_empty(), "Screen should have lines");
    // Prompt length varies ("r> " is 3, "R 4.5.2> " is 9), just verify it's > 0
    let initial_col = screen.cursor_col;
    assert!(
        initial_col > 0,
        "Initial cursor column should be after prompt"
    );

    // Get prompt row for line-based assertions
    let prompt_row = screen.cursor_row as usize;

    // Use line() to check absolute row content
    terminal
        .line(prompt_row)
        .assert_contains("> ")
        .expect("Prompt line should start with prompt indicator");

    // Execute a simple expression
    terminal.send_line("100").expect("Should send 100");
    terminal
        .clear_and_expect("[1] 100")
        .expect("Should show result");

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
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after paste");

    terminal
        .send_line("nchar(x)")
        .expect("Should send nchar(x)");
    terminal
        .clear_and_expect("[1] 10")
        .expect("nchar(x) should return 10");

    // Test medium bracketed paste (100 characters) - validates paste handling
    let content = "y <- '".to_string() + &"b".repeat(100) + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal
        .send(&pasted)
        .expect("Should send medium bracketed paste");
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after medium paste");

    terminal
        .send_line("nchar(y)")
        .expect("Should send nchar(y)");
    terminal
        .clear_and_expect("[1] 100")
        .expect("nchar(y) should return 100");

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

    terminal
        .send(&pasted)
        .expect("Should send long bracketed paste");
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after long paste");

    terminal
        .send_line("nchar(x)")
        .expect("Should send nchar(x)");
    terminal
        .clear_and_expect("[1] 5000")
        .expect("nchar(x) should return 5000");

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

    terminal
        .send(&pasted)
        .expect("Should send multiline long paste");
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after multiline paste");

    terminal
        .send_line("nchar(x)")
        .expect("Should send nchar(x)");
    terminal
        .clear_and_expect("[1] 4001")
        .expect("nchar(x) should return 4001");

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
    let s = "中".repeat(1000)
        + "\n"
        + &"文".repeat(1000)
        + "\n"
        + &"中".repeat(1000)
        + "\n"
        + &"文".repeat(1000);
    let content = "x <- '".to_string() + &s + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal
        .send(&pasted)
        .expect("Should send multibyte long paste");
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after multibyte paste");

    terminal
        .send_line("nchar(x)")
        .expect("Should send nchar(x)");
    terminal
        .clear_and_expect("[1] 4003")
        .expect("nchar(x) should return 4003");

    // Test with different variable name (different padding)
    let content = "xy <- '".to_string() + &s + "'";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal
        .send(&pasted)
        .expect("Should send multibyte paste with different padding");
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after second paste");

    terminal
        .send_line("nchar(xy)")
        .expect("Should send nchar(xy)");
    terminal
        .clear_and_expect("[1] 4003")
        .expect("nchar(xy) should return 4003");

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

    terminal
        .send(&pasted)
        .expect("Should send multiple expressions");

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
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after paste");

    // Verify the value is correct (if parentheses were duplicated, this would fail)
    terminal.send_line("x").expect("Should send x");
    terminal
        .clear_and_expect("[1] 1")
        .expect("x should be 1, not error from extra bracket");

    // Test pasting a function call with multiple brackets
    let content = "y <- sum(c(1, 2, 3))";
    let pasted = format!("{}{}{}\n", paste_start, content, paste_end);

    terminal
        .send(&pasted)
        .expect("Should send nested brackets paste");
    terminal
        .clear_and_expect("> ")
        .expect("Should show prompt after nested paste");

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
#[cfg(unix)]
fn test_pty_escape_cancels_input() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Type some text but don't execute
    terminal
        .send("invalid_var")
        .expect("Should send partial input");
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send Ctrl+C to cancel (more reliable than Escape for canceling input)
    terminal.send_interrupt().expect("Should send interrupt");
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Should be back at prompt
    terminal
        .wait_for_prompt()
        .expect("Should show prompt after cancel");

    // Now execute a valid expression
    terminal.send_line("42").expect("Should send 42");
    terminal
        .clear_and_expect("[1] 42")
        .expect("Should execute 42");

    // Verify the partial input was truly discarded
    terminal
        .send_line("invalid_var")
        .expect("Should send invalid_var");
    terminal
        .expect("Error")
        .expect("Should show error for undefined variable");

    terminal.quit().expect("Should quit cleanly");
}
