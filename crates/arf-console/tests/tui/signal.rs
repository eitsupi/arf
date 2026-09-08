//! External interrupt and termination regression tests.
//!
//! The common cases use Ctrl+C delivered through the ConPTY/PTY input path so
//! they exercise the user-visible interrupt behavior on every supported OS.
//! Unix-only cases additionally use `kill(2)` instead of tui-test's `Signal`
//! operation: the latter writes Ctrl+C to the PTY for `INT` and calls the
//! portable-pty child kill path for termination, neither of which is an
//! external Unix signal delivered to arf.

use super::support::{DEFAULT_CONFIG, ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
#[cfg(unix)]
use anyhow::Context;
#[cfg(unix)]
use anyhow::bail;
use anyhow::{Result, ensure};
#[cfg(unix)]
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const SPAM_INTERVAL: Duration = Duration::from_millis(90);
const SPAM_COUNT: usize = 15;

#[cfg(unix)]
struct PtyChildGuard {
    child: Option<Box<dyn Child + Send + Sync>>,
}

#[cfg(unix)]
impl Drop for PtyChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
    // tui-test beta.3 exposes only a numeric exit code in State, which loses
    // the distinction between a signal and an ordinary failure. Use its
    // portable-pty layer directly here so ExitStatus::signal() remains
    // available for the exact default-disposition assertion.
    let work = tempfile::tempdir()?;
    let config = work.path().join("config.toml");
    std::fs::write(&config, DEFAULT_CONFIG)?;
    let history = work.path().join("history");
    std::fs::create_dir(&history)?;

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 32,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_arf"));
    command.env_remove("ARF_R_HOME");
    command.env_remove("ARF_R_VERSION");
    command.args([
        "--vanilla",
        "--no-r-source-overrides",
        "--config",
        config.to_string_lossy().as_ref(),
        "--history-dir",
        history.to_string_lossy().as_ref(),
        "--no-auto-match",
        "--no-completion",
    ]);
    command.cwd(work.path());
    let child = pair.slave.spawn_command(command)?;
    let mut child = PtyChildGuard { child: Some(child) };
    let _writer = pair.master.take_writer()?;
    let mut reader = pair.master.try_clone_reader()?;
    drop(pair.slave);
    let (ready_tx, ready_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut output = String::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = ready_tx.send(Err(format!(
                        "PTY closed before R became ready; output={output}"
                    )));
                    return;
                }
                Ok(count) => {
                    output.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if output.contains("is ready.") {
                        let _ = ready_tx.send(Ok(()));
                        return;
                    }
                }
                Err(_) => {
                    let _ = ready_tx.send(Err(format!("PTY reader failed; output={output}")));
                    return;
                }
            }
        }
    });
    ready_rx
        .recv_timeout(Duration::from_secs(30))
        .context("timed out waiting for R startup readiness")?
        .map_err(anyhow::Error::msg)?;
    let pid = child
        .child
        .as_ref()
        .and_then(|child| child.process_id())
        .context("missing arf PID")? as libc::pid_t;
    if let Some(status) = child.child.as_mut().expect("guard has child").try_wait()? {
        bail!("arf exited before SIGTERM: {status:?}");
    }
    ensure!(
        unsafe { libc::kill(pid, libc::SIGTERM) } == 0,
        "failed to send SIGTERM to {pid}: {}",
        std::io::Error::last_os_error()
    );
    let status = child.child.as_mut().expect("guard has child").wait()?;
    child.child.take();
    let expected_signal_name =
        unsafe { std::ffi::CStr::from_ptr(libc::strsignal(libc::SIGTERM)).to_string_lossy() };
    ensure!(
        status.signal() == Some(expected_signal_name.as_ref()),
        "SIGTERM did not use its default disposition: {status:?}"
    );
    Ok(())
}
