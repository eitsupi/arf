//! External signal PTY integration tests for arf.
//!
//! These tests deliver SIGINT/SIGTERM via kill(2), which exercises a
//! different code path from `send_interrupt()`: that writes `\x03` into the
//! PTY and relies on the line discipline to raise SIGINT (covered by
//! `test_pty_interrupt_computation`), while an external kill reaches the
//! process regardless of terminal mode. They are regression tests for the
//! Unix SIGINT handler: interrupt forwarding to R, dropping signals while
//! waiting for console input, the startup-profile window, and keeping the
//! SIGINT-only sigaction from capturing SIGTERM.
//!
//! All tests are Unix-only: they use kill(1), and the common PTY harness is
//! Unix-only anyway.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Milliseconds between signals when spamming, matching the manual
/// verification procedure (rapid, but not signal-coalescing fast).
#[cfg(unix)]
const SPAM_INTERVAL_MS: u64 = 90;

/// Number of signals sent by the spam tests.
#[cfg(unix)]
const SPAM_COUNT: u32 = 15;

/// Send a signal to the arf process via kill(1).
#[cfg(unix)]
fn send_signal(terminal: &Terminal, signal: &str) {
    let pid = terminal
        .process_id()
        .expect("Child process ID should be available");
    let status = std::process::Command::new("kill")
        .args(["-s", signal, &pid.to_string()])
        .status()
        .expect("kill(1) should be runnable");
    assert!(status.success(), "kill -s {signal} {pid} failed");
}

/// Send `SPAM_COUNT` signals spaced `SPAM_INTERVAL_MS` apart.
#[cfg(unix)]
fn spam_signal(terminal: &Terminal, signal: &str) {
    for _ in 0..SPAM_COUNT {
        send_signal(terminal, signal);
        std::thread::sleep(std::time::Duration::from_millis(SPAM_INTERVAL_MS));
    }
}

/// Test that an externally delivered SIGINT interrupts a running evaluation.
///
/// The `print()` marker guarantees the evaluation has started before the
/// signal is sent. If the interrupt did not work, `Sys.sleep(30)` would
/// outlast the 15s expect timeout on the follow-up evaluation.
#[test]
#[cfg(unix)]
fn test_pty_external_sigint_interrupts_evaluation() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");
    terminal.wait_for_prompt().expect("Should show prompt");

    terminal
        .send_line("{ print(8100 + 62); Sys.sleep(30) }")
        .expect("Should start evaluation");
    terminal
        .expect("[1] 8162")
        .expect("Evaluation should have started");

    send_signal(&terminal, "INT");

    terminal
        .send_line("sum(1:10)")
        .expect("Should send follow-up expression");
    terminal
        .expect("[1] 55")
        .expect("Session should return to prompt and evaluate normally");

    terminal.quit().expect("Should quit cleanly");
}

/// Test that rapid external SIGINTs at the prompt neither kill the session
/// nor leave a stale interrupt that aborts the next evaluation.
#[test]
#[cfg(unix)]
fn test_pty_external_sigint_spam_at_prompt() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");
    terminal.wait_for_prompt().expect("Should show prompt");

    spam_signal(&terminal, "INT");

    // No fixed settle delay: a stale interrupt flag would abort this
    // evaluation at its start, and `expect`'s own retry loop (up to
    // DEFAULT_TIMEOUT_MS) absorbs any lag in handling the last signal.
    terminal
        .send_line("sum(1:10)")
        .expect("Should send expression after spam");
    terminal
        .expect("[1] 55")
        .expect("Evaluation after prompt-time SIGINT spam should succeed");

    terminal.quit().expect("Should quit cleanly");
}

/// Test that rapid external SIGINTs during an evaluation interrupt it once
/// and leave a working session (the remaining signals land at the prompt and
/// must be dropped there).
#[test]
#[cfg(unix)]
fn test_pty_external_sigint_spam_during_evaluation() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");
    terminal.wait_for_prompt().expect("Should show prompt");

    terminal
        .send_line("{ print(9000 + 100); Sys.sleep(30) }")
        .expect("Should start evaluation");
    terminal
        .expect("[1] 9100")
        .expect("Evaluation should have started");

    spam_signal(&terminal, "INT");

    // No fixed settle delay here either; see the prompt-time spam test above.
    terminal
        .send_line("sum(2:10)")
        .expect("Should send expression after spam");
    terminal
        .expect("[1] 54")
        .expect("Evaluation after SIGINT spam should succeed");

    terminal.quit().expect("Should quit cleanly");
}

/// Test that SIGINT during startup profile evaluation interrupts the profile
/// and still reaches a working prompt.
///
/// Startup profiles run inside setup_Rmainloop, before the REPL loop exists.
/// Before the fix that installs the SIGINT handler ahead of R initialization,
/// this scenario terminated the whole process (default disposition).
///
/// The end marker is assembled with `sep` so the profile source echoed or
/// logged anywhere can never contain the literal marker string.
#[test]
#[cfg(unix)]
fn test_pty_sigint_interrupts_startup_profile() {
    let dir = tempfile::tempdir().expect("Should create temp dir");
    let profile_path = dir.path().join("slow_profile.R");
    std::fs::write(
        &profile_path,
        r"cat('profile-', 'begin\n', sep = '')
Sys.sleep(30)
cat('profile-', 'end\n', sep = '')
",
    )
    .expect("Should write profile");

    let mut terminal = Terminal::spawn_with_args_and_env(
        &["--no-auto-match"],
        &[("R_PROFILE_USER", profile_path.to_str().unwrap())],
    )
    .expect("Failed to spawn arf");

    terminal
        .expect("profile-begin")
        .expect("Profile should have started");

    send_signal(&terminal, "INT");

    terminal
        .wait_for_prompt()
        .expect("Should reach the prompt after interrupting the profile");

    terminal
        .send_line("1 + 1")
        .expect("Should send expression after startup");
    terminal
        .expect("[1] 2")
        .expect("Session should work after interrupted profile");

    let output = terminal.get_output().expect("Should read output");
    assert!(
        !output.contains("profile-end"),
        "Profile should have been interrupted before completing. Output:\n{output}"
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test that SIGTERM still terminates the session.
///
/// The interactive REPL registers a SIGINT-only sigaction; SIGTERM must keep
/// its default disposition (the workspace enables ctrlc's `termination`
/// feature for headless mode, which would capture SIGTERM if it were used
/// for the interactive handler).
#[test]
#[cfg(unix)]
fn test_pty_sigterm_terminates_session() {
    let mut terminal =
        Terminal::spawn_with_args(&["--no-auto-match"]).expect("Failed to spawn arf");
    terminal.wait_for_prompt().expect("Should show prompt");

    send_signal(&terminal, "TERM");

    let status = terminal
        .wait_for_exit_status(std::time::Duration::from_secs(5))
        .expect("SIGTERM should terminate the session");
    // `portable_pty::ExitStatus` only exposes the `strsignal()`-derived name,
    // not the raw signal number, so look up this process's own description
    // for SIGTERM rather than hardcoding one: `strsignal` output depends on
    // the OS and locale, and this test runs on both Linux and macOS CI. A
    // bare `signal().is_some()` check would also pass if SIGTERM handling
    // somehow crashed the process with a different signal instead of
    // terminating it, so compare against the specific signal name.
    let expected_signal_name = unsafe {
        std::ffi::CStr::from_ptr(libc::strsignal(libc::SIGTERM))
            .to_string_lossy()
            .into_owned()
    };
    assert_eq!(
        status.signal(),
        Some(expected_signal_name.as_str()),
        "Process should have been terminated by SIGTERM's default disposition, \
         got exit status: {status}"
    );

    terminal.quit().expect("Cleanup should succeed");
}
