//! Isolated real arf sessions; terminal transport and emulation belong to tui-test.

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempDir};
use tui_test::{
    AutomaticRecording, AutomaticRecordingMode, KeyAction, OpenOptions, Operation, OperationResult,
    RunOptions, Session, State,
};

const WAIT: Duration = Duration::from_secs(30);
pub const PROMPT: &str = "ARF>";
pub const ERROR_PROMPT: &str = "ERR ARF>";

pub struct Terminal {
    session: Session,
    artifacts: PathBuf,
    work: TempDir,
    pid: Option<u32>,
}

impl Terminal {
    fn stage(&self, name: &str) -> Result<()> {
        eprintln!("tui-test: {name} (artifacts: {})", self.artifacts.display());
        fs::write(self.artifacts.join("stage.txt"), name)?;
        Ok(())
    }

    pub fn execute(&self, operation: Operation) -> Result<OperationResult> {
        Ok(self.session.execute(operation)?)
    }

    pub fn state(&self) -> Result<State> {
        match self.execute(Operation::State)? {
            OperationResult::State(state) => Ok(state),
            _ => bail!("unexpected state response"),
        }
    }

    fn current_line(&self, state: &State) -> Result<String> {
        match self.execute(Operation::Cells {
            x: 0,
            y: state.cursor.y,
            w: state.cols,
            h: 1,
        })? {
            OperationResult::Cells(cells) => Ok(cells.into_iter().map(|cell| cell.char).collect()),
            _ => bail!("unexpected cells response"),
        }
    }

    pub fn wait_for(
        &self,
        description: &str,
        predicate: impl Fn(&State, &str) -> bool,
    ) -> Result<()> {
        self.stage(description)?;
        let started = Instant::now();
        loop {
            let state = self.state()?;
            let line = self.current_line(&state)?;
            // Keep the last observation if a later PTY operation blocks.
            fs::write(
                self.artifacts.join("state.json"),
                serde_json::to_vec_pretty(&state)?,
            )?;
            ensure!(
                state.exited.is_none(),
                "arf exited waiting for {description}: {state:?}"
            );
            if predicate(&state, &line) {
                return Ok(());
            }
            ensure!(
                started.elapsed() < WAIT,
                "timed out waiting for {description}: {state:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn wait_for_prompt(&self, output: Option<&str>, prompt: &str) -> Result<()> {
        // A new, independent output line plus the cursor's prompt line avoids
        // matching old prompts and source echoes. Each command's marker must be
        // unique within the session. For repeated output use recording checkpoints.
        // arf emits no shell integration: WaitReady/WaitCommand are not suitable.
        // beta.3's locator also loses wide/combining characters; use text/cells.
        self.wait_for("output and input prompt", |state, line| {
            line.trim_end() == prompt
                && usize::from(state.cursor.x) == prompt.len() + 1
                && output.is_none_or(|text| state.text.lines().any(|line| line == text))
        })
    }

    pub fn enter(&self, source: &str) -> Result<()> {
        self.stage("submit input")?;
        self.execute(Operation::Submit {
            data: Some(source.into()),
        })?;
        Ok(())
    }

    pub fn submit(&self, source: &str, output: &str, prompt: &str) -> Result<()> {
        self.enter(source)?;
        self.wait_for_prompt(Some(output), prompt)
    }

    pub fn write(&self, text: &str) -> Result<()> {
        self.execute(Operation::Write { data: text.into() })?;
        Ok(())
    }

    pub fn key(&self, key: &str) -> Result<()> {
        self.execute(Operation::Key {
            keys: vec![key.into()],
            action: KeyAction::Press,
        })?;
        Ok(())
    }

    /// Read output events, including text subsequently erased from the screen.
    pub fn output(&self) -> Result<String> {
        output_events(&self.session.recording()?)
    }

    pub fn checkpoint(&self) -> Result<usize> {
        Ok(self.output()?.len())
    }

    pub fn output_since(&self, checkpoint: usize) -> Result<String> {
        self.output()?
            .get(checkpoint..)
            .map(str::to_owned)
            .context("invalid output checkpoint")
    }

    fn start(&mut self, args: &[&str]) -> Result<()> {
        let config = self.work.path().join("config.toml");
        fs::write(
            &config,
            r#"[prompt]
format = '{status}ARF> '
[prompt.status.symbol]
error = 'ERR '
"#,
        )?;
        let defaults = OpenOptions::default();
        let mut arguments = vec![
            "--vanilla".into(),
            "--no-r-source-overrides".into(),
            "--config".into(),
            config.to_string_lossy().into_owned(),
            "--history-dir".into(),
            self.work
                .path()
                .join("history")
                .to_string_lossy()
                .into_owned(),
        ];
        arguments.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.stage("spawn")?;
        let opened = self.session.run(RunOptions {
            backend: defaults.backend,
            program: env!("CARGO_BIN_EXE_arf").into(),
            args: arguments,
            profile: defaults.profile,
            cols: 100,
            rows: 32,
            cwd: Some(self.work.path().to_string_lossy().into_owned()),
            env: vec![(
                "ARF_IPC_SESSIONS_DIR".into(),
                self.sessions_dir().to_string_lossy().into_owned(),
            )],
            wait_ready: Some(false),
            restart: false,
            timeouts: defaults.timeouts,
            recording: AutomaticRecording {
                mode: AutomaticRecordingMode::Always,
                directory: Some(self.artifacts.clone()),
            },
        })?;
        self.pid = opened.shell_pid;
        self.wait_for_prompt(None, PROMPT)
    }

    fn sessions_dir(&self) -> PathBuf {
        self.work.path().join("sessions")
    }

    /// Use the real CLI's platform-aware IPC transport, targeting only this R process.
    pub fn start_ipc(&self, args: &[&str]) -> Result<IpcCommand> {
        let stdout = NamedTempFile::new_in(self.work.path())?;
        let stderr = NamedTempFile::new_in(self.work.path())?;
        let child = Command::new(env!("CARGO_BIN_EXE_arf"))
            .arg("ipc")
            .args(args)
            .arg("--pid")
            .arg(self.pid.context("missing arf PID")?.to_string())
            .env("ARF_IPC_SESSIONS_DIR", self.sessions_dir())
            .current_dir(self.work.path())
            .stdin(Stdio::null())
            .stdout(stdout.reopen()?)
            .stderr(stderr.reopen()?)
            .spawn()?;
        Ok(IpcCommand {
            child,
            stdout,
            stderr,
        })
    }

    fn quit(&self) -> Result<()> {
        self.stage("normal exit")?;
        self.enter("q('no')")?;
        self.execute(Operation::WaitExit {
            timeout_ms: Some(30_000),
        })?;
        let state = self.state()?;
        fs::write(
            self.artifacts.join("state.json"),
            serde_json::to_vec_pretty(&state)?,
        )?;
        ensure!(state.exited == Some(0), "wrong process exit: {state:?}");
        Ok(())
    }
}

pub struct IpcCommand {
    child: Child,
    stdout: NamedTempFile,
    stderr: NamedTempFile,
}

impl IpcCommand {
    pub fn finish(mut self) -> Result<Value> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                let stdout = fs::read_to_string(self.stdout.path())?;
                let stderr = fs::read_to_string(self.stderr.path())?;
                ensure!(
                    status.success(),
                    "IPC failed: {status}; stdout={stdout}; stderr={stderr}"
                );
                return serde_json::from_str(&stdout)
                    .with_context(|| format!("invalid IPC JSON: {stdout}; stderr={stderr}"));
            }
            ensure!(start.elapsed() < WAIT, "IPC client timed out");
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for IpcCommand {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn output_events(recording: &str) -> Result<String> {
    let mut lines = recording.lines();
    let header: Value = serde_json::from_str(lines.next().context("missing recording header")?)?;
    ensure!(header.is_object(), "invalid recording header");
    let mut output = String::new();
    for line in lines {
        let event: Value = serde_json::from_str(line)?;
        let fields = event
            .as_array()
            .context("recording event must be an array")?;
        ensure!(fields.len() == 3, "invalid recording event: {event}");
        if fields[1] == "o" {
            output.push_str(
                fields[2]
                    .as_str()
                    .context("output event must contain text")?,
            );
        }
    }
    Ok(output)
}

/// Bound the whole case through close/drop, not just polling calls. The watchdog
/// never touches terminal locks. This test binary fails if any case hangs.
pub fn run_case(
    name: &str,
    args: &[&str],
    test: impl FnOnce(&Terminal) -> Result<()>,
) -> Result<()> {
    let root = std::env::var_os("ARF_TUI_TEST_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root)?;
    let artifacts = tempfile::Builder::new()
        .prefix(&format!("arf-tui-{name}-"))
        .tempdir_in(root)?
        .keep();
    let (_watchdog, deadline) = mpsc::channel::<()>();
    let diagnostics = artifacts.clone();
    thread::spawn(move || {
        if matches!(
            deadline.recv_timeout(Duration::from_secs(180)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            let message = format!(
                "tui-test exceeded 180s; diagnostics: {}\n",
                diagnostics.display()
            );
            let _ = fs::write(diagnostics.join("timeout.txt"), &message);
            // Bypass libtest's per-test capture: process::exit does not flush it.
            let _ = std::io::stderr().lock().write_all(message.as_bytes());
            std::process::exit(1);
        }
    });
    let mut terminal = Terminal {
        session: Session::new(name),
        artifacts,
        work: tempfile::tempdir()?,
        pid: None,
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        terminal.start(args)?;
        test(&terminal)?;
        terminal.quit()
    }));
    match &result {
        Ok(Err(error)) => fs::write(terminal.artifacts.join("failure.txt"), format!("{error:#}"))?,
        Err(_) => fs::write(
            terminal.artifacts.join("failure.txt"),
            "test panicked; see cargo test output",
        )?,
        _ => {}
    }
    terminal.stage("close")?;
    let close = terminal.session.close();
    let result = match result {
        Ok(result) => result,
        Err(panic) => resume_unwind(panic),
    };
    result.with_context(|| format!("{name}; diagnostics: {}", terminal.artifacts.display()))?;
    close.context("close PTY")?;
    terminal.stage("passed")?;
    Ok(())
}

#[test]
fn recording_concatenates_output_without_input_or_resize_events() -> Result<()> {
    let recording = r#"{"version":3}
[0,"o","日"]
[0,"i","secret"]
[0,"r","80x24"]
[0,"o","本語\r\u001b[2K"]
"#;
    ensure!(output_events(recording)? == "日本語\r\x1b[2K");
    Ok(())
}

#[test]
fn malformed_recordings_fail_instead_of_dropping_output() {
    for recording in [
        "",
        r#"{}
[0,"o",123]
"#,
        r#"{}
[0,"o"]
"#,
        r#"{}
truncated"#,
    ] {
        assert!(output_events(recording).is_err(), "accepted {recording:?}");
    }
}
