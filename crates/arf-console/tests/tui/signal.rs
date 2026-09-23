//! External interrupt and termination regression tests.
//!
//! The common cases use Ctrl+C delivered through the ConPTY/PTY input path so
//! they exercise the user-visible interrupt behavior on every supported OS.
//! Unix-only cases additionally use `kill(2)` instead of tui-test's `Signal`
//! operation: the latter writes Ctrl+C to the PTY for `INT` and calls the
//! portable-pty child kill path for termination, neither of which is an
//! external Unix signal delivered to arf.

use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Context, Result, bail, ensure};
#[cfg(unix)]
use portable_pty::{PtySize, native_pty_system};
#[cfg(unix)]
use std::ffi::CStr;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Instant;
use std::{thread, time::Duration};

const SPAM_INTERVAL: Duration = Duration::from_millis(90);
const SPAM_COUNT: usize = 15;

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
fn set_nonblocking(fd: std::os::unix::io::RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    ensure!(
        flags >= 0,
        "failed to read PTY master flags: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "failed to set PTY master nonblocking: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

#[cfg(unix)]
fn spawn_pty_reader(
    mut reader: Box<dyn Read + Send>,
    shutdown: Arc<AtomicBool>,
    startup_artifact: PathBuf,
    ready_tx: mpsc::Sender<std::result::Result<String, String>>,
    would_block_tx: Option<mpsc::Sender<()>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut output = String::new();
        let mut buffer = [0_u8; 1024];
        let mut would_block_tx = would_block_tx;
        loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }
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
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(tx) = would_block_tx.take() {
                        let _ = tx.send(());
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ =
                        ready_tx.send(Err(format!("PTY reader failed: {error}; output={output}")));
                    return;
                }
            }
        }
    })
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
fn nonblocking_pty_reader_shutdown_allows_join_without_eof() -> Result<()> {
    let artifacts = retained_artifacts("nonblocking-pty-reader")?;
    let stages = StageTracker::new(artifacts.clone());
    let (watchdog_done, watchdog) = mpsc::channel::<()>();
    let watchdog_stages = stages.current.clone();
    let watchdog_artifacts = artifacts.clone();
    thread::spawn(move || {
        if matches!(
            watchdog.recv_timeout(Duration::from_secs(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            let stage = watchdog_stages
                .lock()
                .map(|stage| stage.clone())
                .unwrap_or_else(|_| "unknown".to_owned());
            let message = format!(
                "tui-test nonblocking_pty_reader_shutdown_allows_join_without_eof exceeded 30s at stage {stage}; diagnostics: {}\n",
                watchdog_artifacts.display()
            );
            let _ = std::fs::write(watchdog_artifacts.join("timeout.txt"), &message);
            let _ = std::io::stderr().lock().write_all(message.as_bytes());
            std::process::exit(1);
        }
    });

    let result = (|| {
        stages.stage("openpty")?;
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 32,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        stages.stage("configure-reader")?;
        let master_fd = pair
            .master
            .as_raw_fd()
            .context("PTY master has no raw fd")?;
        set_nonblocking(master_fd)?;
        let reader = pair.master.try_clone_reader()?;
        let artifact_dir = tempfile::tempdir()?;
        let startup_artifact = artifact_dir.path().join("startup-output.txt");
        std::fs::write(&startup_artifact, "")?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let (would_block_tx, would_block_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let reader_thread = spawn_pty_reader(
            reader,
            Arc::clone(&shutdown),
            startup_artifact,
            ready_tx,
            Some(would_block_tx),
        );

        // Keep the slave open so the reader has neither output nor EOF to
        // finish on; shutdown must be what makes the nonblocking reader exit.
        let assertion_result = (|| {
            stages.stage("wait-would-block")?;
            let would_block_result = would_block_rx
                .recv_timeout(Duration::from_secs(1))
                .context("nonblocking PTY reader did not observe WouldBlock");
            let ready_result = match ready_rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => Ok(()),
                Ok(message) => bail!(
                    "PTY reader sent a readiness or error message before shutdown: {message:?}"
                ),
                Err(mpsc::TryRecvError::Disconnected) => {
                    bail!("PTY reader exited before shutdown")
                }
            };
            would_block_result?;
            ready_result
        })();
        let stop_reader_stage = stages.stage("stop-reader");
        shutdown.store(true, Ordering::Relaxed);
        let drop_pty_stage = stages.stage("drop-pty");
        drop(pair.master);
        let join_reader_stage = stages.stage("join-reader");
        let reader_result = reader_thread
            .join()
            .map_err(|_| anyhow::anyhow!("PTY reader thread panicked"));
        drop(pair.slave);
        drop(artifact_dir);

        let ready_channel_result: Result<()> = match ready_rx.try_recv() {
            Err(mpsc::TryRecvError::Disconnected) => Ok(()),
            Ok(message) => {
                bail!("PTY reader sent a readiness or error message during shutdown: {message:?}")
            }
            Err(mpsc::TryRecvError::Empty) => {
                bail!("PTY reader channel remained connected after join")
            }
        };
        let cleanup_result = stop_reader_stage
            .and(drop_pty_stage)
            .and(join_reader_stage)
            .and(reader_result)
            .and(ready_channel_result);
        cleanup_result?;
        assertion_result?;
        Ok(())
    })();
    let passed_stage = if result.is_ok() {
        stages.stage("passed")
    } else {
        Ok(())
    };
    drop(watchdog_done);
    result?;
    passed_stage?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn external_sigterm_uses_default_termination_disposition() -> Result<()> {
    // Keep SIGTERM external to tui-test's Signal operation, which uses the
    // PTY input path for Ctrl+C and portable-pty's child kill path otherwise.
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
