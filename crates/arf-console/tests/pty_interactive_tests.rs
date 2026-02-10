//! Interactive feature PTY integration tests for arf.
//!
//! These tests cover readline, askpass, shell mode, system command, help browser,
//! history schema pager (display, copy, mouse scroll), and history browser
//! (persistence, navigation, WAL corruption regression).
//!
//! All tests are Unix-only because crossterm's `cursor::position()` uses WinAPI
//! on Windows, which doesn't work correctly inside ConPTY.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Test R's readline() function in interactive mode.
///
/// This tests that R's readline() can prompt for input and receive it.
/// The readline() function displays a prompt and waits for user input.
///
/// Port of: radian/tests/test_readline.py::test_readline
#[test]
#[cfg(unix)]
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
    terminal
        .expect("input> ")
        .expect("Should see readline prompt");

    // Provide input to readline
    terminal
        .send_line("user_answer")
        .expect("Should send readline input");

    // The readline result should be returned
    terminal
        .expect(r#""user_answer""#)
        .expect("readline should return the input");

    terminal.quit().expect("Should quit cleanly");
}

/// Test askpass package integration for password prompts.
///
/// This tests that:
/// 1. The askpass package can prompt for input and receive it
/// 2. The password input is NOT echoed back in the terminal output
///
/// The custom askpass handler (set via `options(askpass = ...)`) reads directly
/// from `/dev/tty` with echo disabled, bypassing reedline which would otherwise
/// echo the password in plaintext.
///
/// Note: This test requires the askpass package to be installed.
/// Run: install.packages("askpass") to enable this test.
///
/// Port of: radian/tests/test_readline.py::test_askpass
// TODO: Run this test after installing askpass package:
//   1. R -e "install.packages('askpass')"
//   2. cargo test test_pty_askpass -- --ignored
#[test]
#[cfg(unix)]
#[ignore] // Requires askpass package - run with: cargo test -- --ignored
fn test_pty_askpass() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Check if askpass is available
    terminal
        .send_line("requireNamespace('askpass', quietly = TRUE)")
        .expect("Should check askpass");
    terminal
        .expect("TRUE")
        .expect("askpass package should be available");

    // Clear buffer before askpass to isolate output
    terminal.clear_buffer().expect("Should clear buffer");

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

    // The askpass result should be returned as a string
    terminal
        .expect(r#""secret_answer""#)
        .expect("askpass should return the input");

    // Verify the password was NOT echoed in the terminal output.
    // The return value "secret_answer" (in quotes) should appear exactly once
    // in the output. If the password was echoed, it would appear without quotes
    // as well. We check that bare `secret_answer` (without surrounding quotes)
    // does not appear outside of the R return value.
    let output = terminal.get_output().expect("Should get output");
    let bare_occurrences = output.matches("secret_answer").count();
    let quoted_occurrences = output.matches(r#""secret_answer""#).count();
    assert_eq!(
        bare_occurrences, quoted_occurrences,
        "Password should NOT be echoed in plaintext. \
         Found {} bare occurrences vs {} quoted occurrences in output:\n{}",
        bare_occurrences, quoted_occurrences, output
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test shell mode via :shell command.
///
/// This tests entering shell mode with :shell, executing shell commands,
/// and returning to R mode with :r.
#[test]
#[cfg(unix)]
fn test_pty_shell_mode() {
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Enter shell mode
    terminal.send_line(":shell").expect("Should send :shell");
    terminal
        .expect("Shell mode enabled")
        .expect("Should show shell mode message");

    // Prompt should now show shell format (e.g., "[bash] $ " or "[sh] $ ")
    // Wait for the shell prompt to appear
    terminal
        .expect("] $")
        .expect("Should show shell mode prompt");

    // Execute a shell command
    terminal
        .send_line("echo hello")
        .expect("Should send shell command");
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
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

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
    terminal
        .clear_and_expect("[1] 100")
        .expect("Should evaluate R code");

    terminal.quit().expect("Should quit cleanly");
}

/// Test Ctrl+C exits shell mode.
///
/// This tests that pressing Ctrl+C while in shell mode returns to R mode.
#[test]
#[cfg(unix)]
fn test_pty_shell_mode_ctrl_c_exit() {
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

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
    terminal
        .clear_and_expect("[1] 200")
        .expect("Should evaluate R code");

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
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Send :h command to open help browser
    terminal.send_line(":h").expect("Should send :h command");

    // Wait for browser to appear - it should show the header
    std::thread::sleep(std::time::Duration::from_millis(500));
    terminal
        .expect("Help Search")
        .expect("Should show help browser header");

    // Press Esc to exit the browser
    terminal
        .send("\x1b")
        .expect("Should send Esc to exit browser");

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
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

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
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

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
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

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

/// Test that history persists across sessions.
///
/// This test verifies:
/// 1. Commands executed in session 1 are saved to history
/// 2. Session 2 can see history from session 1 in the browser
///
/// This is a regression test to ensure history is properly persisted.
#[test]
#[cfg(unix)]
fn test_pty_history_browser_persists_across_sessions() {
    // Create a temporary directory for history
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let history_dir = temp_dir.path().to_string_lossy().to_string();

    // Session 1: Execute commands and exit
    {
        let mut terminal = Terminal::spawn_with_args(&[
            "--no-auto-match",
            "--no-completion",
            "--history-dir",
            &history_dir,
        ])
        .expect("Failed to spawn arf (session 1)");

        terminal.wait_for_prompt().expect("Should show prompt");

        // Execute a distinctive command that we can search for later
        terminal
            .send_line("unique_test_value_12345 <- 999")
            .expect("Should send assignment");
        terminal
            .clear_and_expect("unique_test_value_12345")
            .expect("Should see variable name");

        terminal.wait_for_prompt().expect("Should show prompt");

        // Exit cleanly to ensure history is flushed
        terminal.quit().expect("Should quit cleanly (session 1)");
    }

    // Session 2: Open history browser and verify session 1's command is visible
    {
        let mut terminal = Terminal::spawn_with_args(&[
            "--no-auto-match",
            "--no-completion",
            "--history-dir",
            &history_dir,
        ])
        .expect("Failed to spawn arf (session 2)");

        terminal.wait_for_prompt().expect("Should show prompt");

        // Open history browser
        terminal
            .send_line(":history browse")
            .expect("Should send :history browse");

        terminal
            .expect("History Browser")
            .expect("Should show history browser header");

        // Verify the command from session 1 is visible
        terminal
            .expect("unique_test_value_12345")
            .expect("Should see command from session 1 in history browser");

        // Exit browser and wait for prompt to return
        terminal.send("q").expect("Should send q to exit browser");
        terminal
            .clear_and_expect("> ")
            .expect("Should return to prompt after browser exit");

        terminal.quit().expect("Should quit cleanly (session 2)");
    }
}

/// Test history browser with :history browse command.
///
/// This test verifies:
/// 1. The history browser opens with the correct header
/// 2. Navigation and UI work correctly
/// 3. The browser can be exited with 'q'
/// 4. After closing browser, history can be written and browser reopened
///    (regression test for WAL database corruption issue)
#[test]
#[cfg(unix)]
fn test_pty_history_browser() {
    // Create a temporary directory for history
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let history_dir = temp_dir.path().to_string_lossy().to_string();

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--no-completion",
        "--history-dir",
        &history_dir,
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // First, add some history entries by running commands
    terminal
        .send_line("1 + 1")
        .expect("Should send R expression");
    terminal
        .clear_and_expect("[1] 2")
        .expect("Should evaluate R code");

    terminal.wait_for_prompt().expect("Should show prompt");
    terminal
        .send_line("print('hello')")
        .expect("Should send print command");
    terminal
        .clear_and_expect("[1] \"hello\"")
        .expect("Should print hello");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Open history browser (first time)
    terminal
        .send_line(":history browse")
        .expect("Should send :history browse");

    terminal
        .expect("History Browser")
        .expect("Should show history browser header");

    // Should show [R] mode indicator
    terminal
        .expect("[R]")
        .expect("Should show R mode indicator");

    // Press 'q' to exit the browser and wait for prompt to return
    terminal.send("q").expect("Should send q to exit browser");
    terminal
        .clear_and_expect("> ")
        .expect("Should return to prompt after browser exit");

    // Execute another command (this writes to history database)
    terminal.send_line("42").expect("Should send R expression");
    terminal
        .clear_and_expect("[1] 42")
        .expect("Should evaluate R code after browser exit");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Open history browser again (regression test for WAL corruption)
    // This previously failed with "database disk image is malformed" error
    terminal
        .send_line(":history browse")
        .expect("Should send :history browse again");

    terminal
        .expect("History Browser")
        .expect("Should show history browser header on second open");

    // Exit browser and wait for prompt to return
    terminal.send("q").expect("Should send q to exit browser");
    terminal
        .clear_and_expect("> ")
        .expect("Should return to prompt after second browser exit");

    // Verify we're back at prompt and can still execute commands
    terminal.send_line("99").expect("Should send R expression");
    terminal
        .clear_and_expect("[1] 99")
        .expect("Should evaluate R code after second browser exit");

    terminal.quit().expect("Should quit cleanly");
}
