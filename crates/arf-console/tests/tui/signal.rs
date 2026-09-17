//! External interrupt and termination regression tests.
//!
//! The common cases use Ctrl+C delivered through the ConPTY/PTY input path so
//! they exercise the user-visible interrupt behavior on every supported OS.
//! Unix-only cases additionally use `kill(2)` instead of tui-test's `Signal`
//! operation: the latter writes Ctrl+C to the PTY for `INT` and calls the
//! portable-pty child kill path for termination, neither of which is an
//! external Unix signal delivered to arf.

#[cfg(unix)]
use super::support::DEFAULT_CONFIG;
use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
#[cfg(unix)]
use anyhow::Context;
#[cfg(unix)]
use anyhow::bail;
use anyhow::{Result, ensure};
#[cfg(unix)]
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

const SPAM_INTERVAL: Duration = Duration::from_millis(90);
const SPAM_COUNT: usize = 15;

#[cfg(unix)]
struct PtyChildGuard {
    child: Option<Box<dyn Child + Send + Sync>>,
    artifacts: PathBuf,
}

#[cfg(unix)]
impl Drop for PtyChildGuard {
    fn drop(&mut self) {
        let Some(child) = &mut self.child else {
            return;
        };

        append_cleanup_diagnostic(&self.artifacts, "cleanup-child: starting");
        match child.try_wait() {
            Ok(Some(status)) => {
                append_cleanup_diagnostic(
                    &self.artifacts,
                    format!("cleanup-child: child was already reaped: {status:?}"),
                );
                return;
            }
            Ok(None) => {}
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
                append_cleanup_diagnostic(
                    &self.artifacts,
                    "cleanup-child: initial try_wait reported ECHILD; assuming child was reaped",
                );
                return;
            }
            Err(error) => append_cleanup_diagnostic(
                &self.artifacts,
                format!("cleanup-child: initial try_wait failed: {error}"),
            ),
        }

        let kill_result = child
            .process_id()
            .map(|pid| unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) })
            .map(|result| {
                if result == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        match kill_result {
            Some(Ok(())) => {
                append_cleanup_diagnostic(&self.artifacts, "cleanup-child: sent SIGKILL")
            }
            Some(Err(error)) if error.raw_os_error() == Some(libc::ESRCH) => {
                append_cleanup_diagnostic(
                    &self.artifacts,
                    "cleanup-child: SIGKILL target was already gone",
                );
            }
            Some(Err(error)) => append_cleanup_diagnostic(
                &self.artifacts,
                format!("cleanup-child: SIGKILL failed: {error}"),
            ),
            None => match child.kill() {
                Ok(()) => append_cleanup_diagnostic(
                    &self.artifacts,
                    "cleanup-child: sent portable-pty kill",
                ),
                Err(error) => append_cleanup_diagnostic(
                    &self.artifacts,
                    format!("cleanup-child: portable-pty kill failed: {error}"),
                ),
            },
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    append_cleanup_diagnostic(
                        &self.artifacts,
                        format!("cleanup-child: reaped after kill: {status:?}"),
                    );
                    return;
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    append_cleanup_diagnostic(
                        &self.artifacts,
                        "cleanup-child: timed out waiting to reap after SIGKILL",
                    );
                    return;
                }
                Err(error) => {
                    append_cleanup_diagnostic(
                        &self.artifacts,
                        format!("cleanup-child: try_wait while reaping failed: {error}"),
                    );
                    return;
                }
            }
        }
    }
}

#[cfg(unix)]
struct StageTracker {
    artifacts: PathBuf,
    current: Arc<Mutex<String>>,
}

#[cfg(unix)]
impl StageTracker {
    fn new(artifacts: PathBuf) -> Self {
        Self {
            artifacts,
            current: Arc::new(Mutex::new("prepare".to_owned())),
        }
    }

    fn stage(&self, name: &str) -> Result<()> {
        if let Ok(mut current) = self.current.lock() {
            *current = name.to_owned();
        }
        eprintln!("tui-test: {name} (artifacts: {})", self.artifacts.display());
        std::fs::write(self.artifacts.join("stage.txt"), name)?;
        Ok(())
    }

    fn append(&self, message: impl AsRef<str>) {
        append_cleanup_diagnostic(&self.artifacts, message.as_ref());
    }
}

#[cfg(unix)]
fn append_cleanup_diagnostic(artifacts: &Path, message: impl AsRef<str>) {
    use std::fs::OpenOptions;
    use std::io::Write;

    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(artifacts.join("cleanup.txt"))
    else {
        return;
    };
    let _ = writeln!(file, "{}", message.as_ref());
}

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
    // tui-test beta.3 exposes only a numeric exit code in State, which loses
    // the distinction between a signal and an ordinary failure. Use its
    // portable-pty layer directly here so ExitStatus::signal() remains
    // available for the exact default-disposition assertion.
    let artifacts = retained_artifacts("external-sigterm")?;
    let stages = StageTracker::new(artifacts.clone());
    let (watchdog_done, watchdog) = mpsc::channel::<()>();
    let watchdog_stages = stages.current.clone();
    let watchdog_artifacts = artifacts.clone();
    thread::spawn(move || {
        if matches!(
            watchdog.recv_timeout(Duration::from_secs(180)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            let stage = watchdog_stages
                .lock()
                .map(|stage| stage.clone())
                .unwrap_or_else(|_| "unknown".to_owned());
            let message = format!(
                "tui-test external SIGTERM case exceeded 180s at stage {stage}; diagnostics: {}\n",
                watchdog_artifacts.display()
            );
            let _ = std::fs::write(watchdog_artifacts.join("timeout.txt"), &message);
            let _ = std::io::stderr().lock().write_all(message.as_bytes());
            std::process::exit(1);
        }
    });

    stages.stage("prepare")?;
    let work = tempfile::tempdir()?;
    let config = work.path().join("config.toml");
    std::fs::write(&config, DEFAULT_CONFIG)?;
    let history = work.path().join("history");
    std::fs::create_dir(&history)?;

    let pty_system = native_pty_system();
    stages.stage("openpty")?;
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
    stages.stage("spawn")?;
    let child = pair.slave.spawn_command(command)?;
    let mut child = PtyChildGuard {
        child: Some(child),
        artifacts: artifacts.clone(),
    };
    let pid = child
        .child
        .as_ref()
        .and_then(|child| child.process_id())
        .context("missing arf PID")? as libc::pid_t;
    stages.append(format!("pid: {pid}"));
    // This test only observes startup output. Taking the writer can trigger
    // a blocking EOF write when it is dropped on Unix, so leave it untouched.
    let mut reader = pair.master.try_clone_reader()?;
    drop(pair.slave);
    let startup_artifact = artifacts.join("startup-output.txt");
    std::fs::write(&startup_artifact, "")?;
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader_thread = thread::spawn(move || {
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
                    let _ = std::fs::write(&startup_artifact, &output);
                    if output.contains("is ready.") {
                        let _ = ready_tx.send(Ok(output));
                        return;
                    }
                }
                Err(error) => {
                    let _ =
                        ready_tx.send(Err(format!("PTY reader failed: {error}; output={output}")));
                    return;
                }
            }
        }
    });
    let result = (|| {
        stages.stage("wait-ready")?;
        let startup_result = ready_rx
            .recv_timeout(Duration::from_secs(30))
            .context("timed out waiting for R startup readiness")?;
        match startup_result {
            Ok(_) => {}
            Err(error) => {
                bail!("{error}");
            }
        }
        if let Some(status) = child.child.as_mut().expect("guard has child").try_wait()? {
            bail!("arf exited before SIGTERM: {status:?}");
        }
        stages.stage("send-sigterm")?;
        ensure!(
            unsafe { libc::kill(pid, libc::SIGTERM) } == 0,
            "failed to send SIGTERM to {pid}: {}",
            std::io::Error::last_os_error()
        );
        stages.stage("wait-exit")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.child.as_mut().expect("guard has child").try_wait()? {
                break status;
            }
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for arf to terminate after SIGTERM"
            );
            thread::sleep(Duration::from_millis(25));
        };
        stages.append(format!("exit-status: {status:?}"));
        // The child has been reaped, so do not let later assertion failures
        // leave a stale PID for the guard to signal.
        child.child.take();
        let expected_signal_name =
            unsafe { std::ffi::CStr::from_ptr(libc::strsignal(libc::SIGTERM)).to_string_lossy() };
        ensure!(
            status.signal() == Some(expected_signal_name.as_ref()),
            "SIGTERM did not use its default disposition: {status:?}"
        );
        Ok(())
    })();

    let cleanup_stage = stages.stage("cleanup-child");
    drop(child);
    let drop_pty_stage = stages.stage("drop-pty");
    drop(pair.master);
    let reader_cleanup_stage = stages.stage("cleanup-reader");
    let reader_result = if result.is_ok() || reader_thread.is_finished() {
        reader_thread
            .join()
            .map_err(|_| anyhow::anyhow!("PTY reader thread panicked"))
    } else {
        stages.append("test failure left PTY reader unfinished; detaching reader thread");
        drop(reader_thread);
        Ok(())
    };
    drop(work);
    cleanup_stage?;
    drop_pty_stage?;
    reader_cleanup_stage?;
    reader_result?;
    result?;
    stages.stage("passed")?;
    drop(watchdog_done);
    Ok(())
}
