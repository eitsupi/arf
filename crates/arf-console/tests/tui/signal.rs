//! External interrupt and termination regression tests.
//!
//! The common cases use Ctrl+C delivered through the ConPTY/PTY input path so
//! they exercise the user-visible interrupt behavior on every supported OS.
//! Unix-only cases additionally use `kill(2)` instead of tui-test's `Signal`
//! operation: the latter writes Ctrl+C to the PTY for `INT` and calls the
//! portable-pty child kill path for termination, neither of which is an
//! external Unix signal delivered to arf.

use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, ensure};
use std::thread;
use std::time::Duration;

const SPAM_INTERVAL: Duration = Duration::from_millis(90);
const SPAM_COUNT: usize = 15;

#[cfg(unix)]
fn send_signal(terminal: &Terminal, signal: libc::c_int) -> Result<()> {
    let pid = terminal.pid().context("missing arf PID")? as libc::pid_t;
    ensure!(
        unsafe { libc::kill(pid, signal) } == 0,
        "failed to send signal {signal} to {pid}: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

#[cfg(unix)]
fn spam_sigint(terminal: &Terminal) -> Result<()> {
    for _ in 0..SPAM_COUNT {
        send_signal(terminal, libc::SIGINT)?;
        thread::sleep(SPAM_INTERVAL);
    }
    Ok(())
}

fn wait_for_interrupted_prompt(terminal: &Terminal, marker: &str) -> Result<()> {
    terminal.wait_for("prompt after interrupt", |state, line| {
        state.text.contains(marker) && matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
    })
}

fn spam_ctrl_c(terminal: &Terminal) -> Result<()> {
    for _ in 0..SPAM_COUNT {
        terminal.key("Ctrl+C")?;
        thread::sleep(SPAM_INTERVAL);
    }
    Ok(())
}

#[test]
// This is the portable user-input path; the Unix SIGINT profile case below
// remains separate because it exercises an OS signal while R initializes.
fn ctrl_c_interrupts_slow_startup_profile_before_its_end() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("slow-profile.R");
    std::fs::write(
        &profile,
        "cat('CTRL_C_PROFILE_BEGIN\\n')\nSys.sleep(30)\ncat('CTRL_C_PROFILE_END\\n')\n",
    )?;
    run_case_with(
        Terminal::builder("ctrl-c-profile")
            .args(["--no-auto-match"])
            .env("R_PROFILE_USER", profile.to_string_lossy().into_owned())
            .vanilla(false),
        |terminal| {
            let checkpoint = terminal.checkpoint()?;
            terminal.wait_for("startup profile begins", |state, _| {
                state.text.contains("CTRL_C_PROFILE_BEGIN")
            })?;
            terminal.key("Ctrl+C")?;
            terminal.wait_for("prompt after profile Ctrl+C", |state, line| {
                state.text.contains("CTRL_C_PROFILE_BEGIN")
                    && matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
            })?;
            ensure!(
                !terminal
                    .output_since(checkpoint)?
                    .contains("CTRL_C_PROFILE_END"),
                "startup profile reached its end after Ctrl+C"
            );
            terminal.submit("1 + 1", "[1] 2", PROMPT)
        },
    )
}

#[test]
// Keep this separate from the Unix kill(2) case: it covers repeated Ctrl+C
// bytes delivered by the PTY/ConPTY input path on every supported OS.
fn ctrl_c_spam_at_prompt_does_not_interrupt_next_evaluation() -> Result<()> {
    run_case("ctrl-c-prompt-spam", &["--no-auto-match"], |terminal| {
        spam_ctrl_c(terminal)?;
        terminal.submit("sum(1:10)", "[1] 55", PROMPT)
    })
}

#[test]
// This complements the single-key input regression in input.rs and the Unix
// external-signal case by covering repeated PTY/ConPTY interrupts in R.
fn ctrl_c_spam_during_evaluation_interrupts_once_and_recovers() -> Result<()> {
    run_case("ctrl-c-evaluation-spam", &["--no-auto-match"], |terminal| {
        let marker = "CTRL_C_SPAM_STARTED";
        terminal.enter(&format!("cat('{marker}\\n'); Sys.sleep(30)"))?;
        terminal.wait_for("evaluation starts before Ctrl+C spam", |state, _| {
            state.text.contains(marker)
        })?;
        spam_ctrl_c(terminal)?;
        wait_for_interrupted_prompt(terminal, marker)?;
        terminal.submit("sum(2:10)", "[1] 54", PROMPT)
    })
}

#[cfg(unix)]
#[test]
fn external_sigint_interrupts_evaluation_and_repl_recovers() -> Result<()> {
    run_case(
        "external-sigint-evaluation",
        &["--no-auto-match"],
        |terminal| {
            let marker = "EXTERNAL_SIGINT_EVAL_STARTED";
            terminal.enter(&format!("cat('{marker}\\n'); Sys.sleep(30)"))?;
            terminal.wait_for("evaluation starts before external SIGINT", |state, _| {
                state.text.contains(marker)
            })?;
            send_signal(terminal, libc::SIGINT)?;
            wait_for_interrupted_prompt(terminal, marker)?;
            terminal.submit("sum(1:10)", "[1] 55", PROMPT)
        },
    )
}

#[cfg(unix)]
#[test]
fn external_sigint_spam_at_prompt_does_not_interrupt_next_evaluation() -> Result<()> {
    run_case(
        "external-sigint-prompt-spam",
        &["--no-auto-match"],
        |terminal| {
            spam_sigint(terminal)?;
            terminal.submit("sum(1:10)", "[1] 55", PROMPT)
        },
    )
}

#[cfg(unix)]
#[test]
fn external_sigint_spam_during_evaluation_interrupts_once_and_recovers() -> Result<()> {
    run_case(
        "external-sigint-evaluation-spam",
        &["--no-auto-match"],
        |terminal| {
            let marker = "EXTERNAL_SIGINT_SPAM_STARTED";
            terminal.enter(&format!("cat('{marker}\\n'); Sys.sleep(30)"))?;
            terminal.wait_for(
                "evaluation starts before external SIGINT spam",
                |state, _| state.text.contains(marker),
            )?;
            spam_sigint(terminal)?;
            wait_for_interrupted_prompt(terminal, marker)?;
            terminal.submit("sum(2:10)", "[1] 54", PROMPT)
        },
    )
}

#[cfg(unix)]
#[test]
fn external_sigint_interrupts_slow_startup_profile_before_its_end() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("slow-profile.R");
    std::fs::write(
        &profile,
        "cat('EXTERNAL_SIGINT_PROFILE_BEGIN\\n')\nSys.sleep(30)\ncat('EXTERNAL_SIGINT_PROFILE_END\\n')\n",
    )?;
    run_case_with(
        Terminal::builder("external-sigint-profile")
            .args(["--no-auto-match"])
            .env("R_PROFILE_USER", profile.to_string_lossy().into_owned())
            .vanilla(false),
        |terminal| {
            let checkpoint = terminal.checkpoint()?;
            terminal.wait_for("startup profile begins", |state, _| {
                state.text.contains("EXTERNAL_SIGINT_PROFILE_BEGIN")
            })?;
            send_signal(terminal, libc::SIGINT)?;
            terminal.wait_for("prompt after profile SIGINT", |state, line| {
                state.text.contains("EXTERNAL_SIGINT_PROFILE_BEGIN")
                    && matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
            })?;
            ensure!(
                !terminal
                    .output_since(checkpoint)?
                    .contains("EXTERNAL_SIGINT_PROFILE_END"),
                "startup profile reached its end after SIGINT"
            );
            terminal.submit("1 + 1", "[1] 2", PROMPT)?;
            terminal.quit()
        },
    )
}

#[cfg(unix)]
#[test]
fn external_sigterm_uses_default_termination_disposition() -> Result<()> {
    run_case_with(
        Terminal::builder("external-sigterm").args(["--no-auto-match"]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            send_signal(terminal, libc::SIGTERM)?;
            let exit_code = terminal.wait_for_exit()?;
            ensure!(
                exit_code != 0,
                "SIGTERM unexpectedly returned a successful exit status"
            );
            Ok(())
        },
    )
}
