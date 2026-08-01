//! Regression tests for interactive process behavior.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Test that restarting the same R version preserves session environment changes.
#[test]
#[cfg(unix)]
fn test_pty_restart_preserves_session_environment() {
    let mut terminal = Terminal::spawn_with_args(&["--no-auto-match", "--no-completion"])
        .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Read the value back in the same line: waiting for a bare prompt would
    // match the one reedline repaints while echoing the input.
    terminal
        .clear_buffer()
        .expect("Should clear output before setting the sentinel");
    terminal
        .send_line(r#"Sys.setenv(R_LIBS = "arf_restart_test_sentinel"); Sys.getenv("R_LIBS")"#)
        .expect("Should set the environment sentinel");
    terminal
        .expect(r#"[1] "arf_restart_test_sentinel""#)
        .expect("The sentinel should be set before restarting");

    terminal
        .clear_buffer()
        .expect("Should clear output before restart");
    terminal
        .send_line(":restart!")
        .expect("Should send :restart!");
    terminal
        .expect("Restarting R session...")
        .expect("Should announce the restart");
    // Wait for the restarted session's banner rather than a prompt, and do not
    // clear in between: the buffer was cleared before `:restart!`, so this can
    // only be the banner the new session prints. Sending the query earlier
    // risks the outgoing session answering it, which would pass even if the
    // restart had dropped the variable.
    terminal
        .expect("is ready.")
        .expect("Should wait for the restarted session");

    terminal
        .clear_buffer()
        .expect("Should clear output before reading the sentinel");
    terminal
        .send_line(r#"Sys.getenv("R_LIBS")"#)
        .expect("Should read the environment sentinel after restart");
    terminal
        .expect(r#"[1] "arf_restart_test_sentinel""#)
        .expect("The session environment should survive restart");

    terminal.quit().expect("Should quit cleanly");
}
