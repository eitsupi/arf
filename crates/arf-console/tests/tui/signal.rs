//! External interrupt and termination regression tests.
//!
//! The common cases use Ctrl+C delivered through the ConPTY/PTY input path so
//! they exercise the user-visible interrupt behavior on every supported OS.
//! Unix-only cases additionally use `kill(2)` instead of tui-test's `Signal`
//! operation: the latter writes Ctrl+C to the PTY for `INT` and terminates the
//! child for other signals, neither of which sends an external Unix signal to arf.

use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Context, Result, ensure};
#[cfg(unix)]
use std::ffi::CStr;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Instant;
use std::{thread, time::Duration};

const SPAM_INTERVAL: Duration = Duration::from_millis(90);
const SPAM_COUNT: usize = 15;

#[cfg(unix)]
fn retained_artifacts(name: &str) -> Result<PathBuf> {
    let root = std::env::var_os("ARF_TUI_TEST_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&root)?;
    Ok(tempfile::Builder::new()
        .prefix(&format!("arf-tui-{name}-"))
        .tempdir_in(root)?
        .keep())
}

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

fn wait_for_interrupted_prompt(
    terminal: &Terminal,
    marker: &str,
    completion_marker: &str,
) -> Result<()> {
    terminal.wait_for("prompt after interrupt", |state, line| {
        state.text.lines().any(|line| line == marker)
            && matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
            && !state.text.lines().any(|line| line == completion_marker)
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
        let completion_marker = "CTRL_C_SPAM_COMPLETED";
        terminal.enter(&format!(
            r#"cat('{marker}\n'); Sys.sleep(30); cat('{completion_marker}\n')"#,
        ))?;
        terminal.wait_for("evaluation starts before Ctrl+C spam", |state, _| {
            state.text.lines().any(|line| line == marker)
        })?;
        spam_ctrl_c(terminal)?;
        wait_for_interrupted_prompt(terminal, marker, completion_marker)?;
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
            let completion_marker = "EXTERNAL_SIGINT_EVAL_COMPLETED";
            terminal.enter(&format!(
                r#"cat('{marker}\n'); Sys.sleep(30); cat('{completion_marker}\n')"#,
            ))?;
            terminal.wait_for("evaluation starts before external SIGINT", |state, _| {
                state.text.lines().any(|line| line == marker)
            })?;
            send_signal(terminal, libc::SIGINT)?;
            wait_for_interrupted_prompt(terminal, marker, completion_marker)?;
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
            let completion_marker = "EXTERNAL_SIGINT_SPAM_COMPLETED";
            terminal.enter(&format!(
                r#"cat('{marker}\n'); Sys.sleep(30); cat('{completion_marker}\n')"#,
            ))?;
            terminal.wait_for(
                "evaluation starts before external SIGINT spam",
                |state, _| state.text.lines().any(|line| line == marker),
            )?;
            spam_sigint(terminal)?;
            wait_for_interrupted_prompt(terminal, marker, completion_marker)?;
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
    // Keep SIGTERM external to tui-test's Signal operation, which uses the
    // PTY input path for Ctrl+C and child termination otherwise.
    let artifacts = retained_artifacts("external-sigterm")?;
    let builder = Terminal::builder("external-sigterm")
        .args(["--no-auto-match", "--no-completion"])
        .env_remove("ARF_R_HOME")
        .env_remove("ARF_R_VERSION")
        .artifacts(artifacts);
    run_case_with(builder, |terminal| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let state = terminal.state()?;
            ensure!(
                state.exited.is_none(),
                "arf exited before R startup readiness: {state:?}"
            );
            // Preserve the original boundary: R itself must report readiness;
            // the application prompt is deliberately not a substitute.
            if terminal.output()?.contains("is ready.") {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for R startup readiness: {state:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }

        let pid = terminal.pid().context("missing arf PID")? as libc::pid_t;
        ensure!(
            unsafe { libc::kill(pid, libc::SIGTERM) } == 0,
            "failed to send SIGTERM to {pid}: {}",
            std::io::Error::last_os_error()
        );
        terminal.wait_for_exit()?;
        let state = terminal.state()?;
        let expected_signal =
            unsafe { CStr::from_ptr(libc::strsignal(libc::SIGTERM)).to_string_lossy() };
        ensure!(
            state.exit_signal.as_deref() == Some(expected_signal.as_ref()),
            "SIGTERM did not use its default disposition: {state:?}"
        );
        Ok(())
    })
}
