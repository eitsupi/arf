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

    terminal
        .clear_buffer()
        .expect("Should clear output before setting the sentinel");
    terminal
        .send_line(r#"Sys.setenv(R_LIBS = "arf_restart_test_sentinel")"#)
        .expect("Should set the environment sentinel");
    terminal
        .expect("> ")
        .expect("Should return to the prompt after setting the sentinel");

    terminal
        .clear_buffer()
        .expect("Should clear output before restart");
    terminal
        .send_line(":restart!")
        .expect("Should send :restart!");
    terminal
        .expect("Restarting R session...")
        .expect("Should announce the restart");
    // Clearing is right here: it drops the prompt reedline repainted while
    // echoing `:restart!`, so the wait below is for the prompt the restarted
    // session prints. Nothing was sent between the clear and the wait, so the
    // output cannot arrive before the clear.
    terminal
        .clear_and_expect("> ")
        .expect("Should wait for the prompt after restart");

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
