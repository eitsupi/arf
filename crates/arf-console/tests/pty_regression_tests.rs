//! Regression tests for interactive process behavior.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Test that restarting the same R version preserves session environment changes.
#[test]
#[cfg(unix)]
fn test_pty_restart_preserves_session_environment() {
    // This test waits for the restarted session's startup banner, so it needs
    // an isolated config that forces `show_banner = true`: `spawn_with_args`
    // does not isolate the process from the user's real config file, and if
    // the user has `startup.show_banner = false` there, the banner never
    // appears and the wait below times out even though the restart itself
    // worked correctly.
    let config_dir = tempfile::tempdir().expect("Should create a temp config dir");
    let config_path = config_dir.path().join("arf.toml");
    std::fs::write(&config_path, "[startup]\nshow_banner = true\n")
        .expect("Should write the temp config file");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--no-completion",
        "--config",
        config_path
            .to_str()
            .expect("Config path should be valid UTF-8"),
    ])
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
