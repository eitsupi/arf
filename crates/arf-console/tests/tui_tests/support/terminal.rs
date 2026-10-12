use super::builder::{SharedResources, TerminalBuilder};
use super::{POLL_INTERVAL, PROMPT, WAIT};
use crate::common::{poll_until, try_poll_until};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use tempfile::{NamedTempFile, TempDir};
use tui_test::{
    AutomaticRecording, KeyAction, OpenOptions, Operation, OperationResult, RunOptions, Session,
    State,
};

pub struct Terminal {
    pub(super) session: Session,
    pub(super) artifacts: PathBuf,
    pub(super) work: TempDir,
    pub(super) resources: SharedResources,
    pub(super) cwd: PathBuf,
    pub(super) pid: Option<u32>,
    pub(super) closed: bool,
}

impl Terminal {
    pub fn builder(name: impl Into<String>) -> TerminalBuilder {
        TerminalBuilder::new(name)
    }

    pub fn sessions_dir(&self) -> &std::path::Path {
        self.resources.sessions_dir()
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub(super) fn stage(&self, name: &str) -> Result<()> {
        eprintln!("tui-test: {name} (artifacts: {})", self.artifacts.display());
        fs::write(self.artifacts.join("stage.txt"), name)?;
        Ok(())
    }

    pub fn execute(&self, operation: Operation) -> Result<OperationResult> {
        Ok(self.session.execute(operation)?)
    }

    pub fn state(&self) -> Result<State> {
        session_state(&self.session)
    }

    fn current_line(&self, state: &State) -> Result<String> {
        self.screen_line(state.cursor.y, state.cols)
    }

    /// Read one emulated screen row without depending on the cursor position.
    /// Pager headers live above the hidden cursor, so current-line polling is
    /// insufficient when an alternate-screen UI is active.
    pub fn screen_line(&self, y: u16, cols: u16) -> Result<String> {
        match self.execute(Operation::Cells {
            x: 0,
            y,
            w: cols,
            h: 1,
        })? {
            OperationResult::Cells(cells) => Ok(cells.into_iter().map(|cell| cell.char).collect()),
            _ => bail!("unexpected cells response"),
        }
    }

    pub fn screen_cells(&self, y: u16, cols: u16) -> Result<Vec<tui_test::Cell>> {
        match self.execute(Operation::Cells {
            x: 0,
            y,
            w: cols,
            h: 1,
        })? {
            OperationResult::Cells(cells) => Ok(cells),
            _ => bail!("unexpected cells response"),
        }
    }

    pub fn wait_for(
        &self,
        description: &str,
        predicate: impl Fn(&State, &str) -> bool,
    ) -> Result<()> {
        self.wait_for_line(description, |state| self.current_line(state), predicate)
    }

    fn wait_for_line(
        &self,
        description: &str,
        read_line: impl Fn(&State) -> Result<String>,
        predicate: impl Fn(&State, &str) -> bool,
    ) -> Result<()> {
        self.stage(description)?;
        let mut last_state = None;
        let matched = try_poll_until(WAIT, POLL_INTERVAL, || {
            let state = self.state()?;
            let line = read_line(&state)?;
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
                return Ok(Some(()));
            }
            last_state = Some(state);
            Ok(None)
        })?;
        ensure!(
            matched.is_some(),
            "timed out waiting for {description}: {last_state:?}"
        );
        Ok(())
    }

    /// Poll a fixed emulated screen row, which is useful for pager headers
    /// rendered above the hidden cursor.
    pub fn wait_for_screen_line(
        &self,
        description: &str,
        y: u16,
        predicate: impl Fn(&State, &str) -> bool,
    ) -> Result<()> {
        self.wait_for_line(
            description,
            |state| self.screen_line(y, state.cols),
            predicate,
        )
    }

    /// Wait until a restart or R-version switch issued after `checkpoint` has
    /// announced its replacement R session as ready and the prompt is back,
    /// and return the output produced since `checkpoint`.
    // Only the rig project switch binary, which also includes this module, calls it.
    #[allow(dead_code)]
    pub fn wait_for_replacement_prompt(
        &self,
        checkpoint: usize,
        description: &str,
    ) -> Result<String> {
        self.wait_for(description, |state, line| {
            state.exited.is_none()
                && line.trim_end() == PROMPT
                && self.output_since(checkpoint).is_ok_and(|output| {
                    (output.contains("Restarting R session...")
                        || output.contains("Restarting with R "))
                        && output.contains("is ready.")
                })
        })?;
        self.output_since(checkpoint)
    }

    pub fn wait_for_prompt(&self, output: Option<&str>, prompt: &str) -> Result<()> {
        // A new, independent output line plus the cursor's prompt line avoids
        // matching old prompts and source echoes. Each command's marker must be
        // unique within the session. For repeated commands, compare the cursor
        // row before and after submission, then check the result on screen.
        // Recording checkpoints are for stream-only checks such as erased
        // output, complete echoes, and non-leak behavior.
        // arf emits no shell integration: WaitReady/WaitCommand are not suitable.
        // tui-test's text locator currently takes only the first character in each cell
        // and treats empty wide-character continuation cells as spaces, so it
        // can miss wide or combining text; use State.text/cells instead.
        self.wait_for("output and input prompt", |state, line| {
            line.trim_end() == prompt
                && usize::from(state.cursor.x) == prompt.len() + 1
                && output.is_none_or(|text| state.text.lines().any(|line| line == text))
        })
    }

    /// Wait for the first application prompt after spawning the process.
    ///
    /// Spawning and readiness are intentionally separate so callers can
    /// inspect startup output or coordinate several sessions before waiting.
    pub fn wait_for_first_prompt(&self) -> Result<()> {
        self.wait_for_prompt(None, PROMPT)
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

    /// Use the real CLI's platform-aware IPC transport, targeting only this R process.
    pub fn start_ipc(&self, args: &[&str]) -> Result<IpcCommand> {
        self.start_ipc_with_pid(args, self.pid.context("missing arf PID")?)
    }

    /// Use the real CLI IPC transport with a caller-selected process ID.
    ///
    /// Restarting arf keeps the tui-test session alive while the process ID
    /// changes on platforms that spawn a replacement child. Callers can read
    /// the active PID from the process's lifecycle artifact and target that
    /// replacement explicitly.
    pub fn start_ipc_with_pid(&self, args: &[&str], pid: u32) -> Result<IpcCommand> {
        let stdout = NamedTempFile::new_in(self.work.path())?;
        let stderr = NamedTempFile::new_in(self.work.path())?;
        let child = Command::new(env!("CARGO_BIN_EXE_arf"))
            .arg("ipc")
            .args(args)
            .arg("--pid")
            .arg(pid.to_string())
            .env("ARF_IPC_SESSIONS_DIR", self.sessions_dir())
            .current_dir(&self.cwd)
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

    pub fn start_cli_tty(&self, args: &[&str]) -> Result<CliSession> {
        CliSession::new("arf-cli-tty", args, &self.cwd, self.sessions_dir())
    }

    pub fn quit(&self) -> Result<()> {
        self.stage("normal exit")?;
        self.enter("q('no')")?;
        let exit_code = self.wait_for_exit()?;
        ensure!(exit_code == 0, "wrong process exit: {exit_code}");
        Ok(())
    }

    /// Wait for an explicitly requested process exit and return its status.
    ///
    /// This is separate from `quit` so lifecycle tests can exercise Ctrl+D or
    /// startup failures without sending an unconditional R command first.
    pub fn wait_for_exit(&self) -> Result<i32> {
        let state = wait_for_session_exit(&self.session)?;
        fs::write(
            self.artifacts.join("state.json"),
            serde_json::to_vec_pretty(&state)?,
        )?;
        state
            .exited
            .context("process did not report an exit status")
    }
}

/// A short-lived CLI process attached to its own tui-test PTY.
pub struct CliSession {
    session: Session,
}

impl CliSession {
    fn new(
        name: &str,
        args: &[&str],
        cwd: &std::path::Path,
        sessions_dir: &std::path::Path,
    ) -> Result<Self> {
        let session = Session::new(name);
        let defaults = OpenOptions::default();
        session.run(RunOptions {
            backend: defaults.backend,
            program: env!("CARGO_BIN_EXE_arf").into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            profile: defaults.profile,
            cols: defaults.cols,
            rows: defaults.rows,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            env: vec![(
                "ARF_IPC_SESSIONS_DIR".to_owned(),
                sessions_dir.to_string_lossy().into_owned(),
            )],
            wait_ready: Some(false),
            restart: false,
            timeouts: defaults.timeouts,
            recording: AutomaticRecording::default(),
        })?;
        Ok(Self { session })
    }

    pub fn state(&self) -> Result<State> {
        session_state(&self.session)
    }

    pub fn wait_for_exit(&self) -> Result<i32> {
        wait_for_session_exit(&self.session)?
            .exited
            .context("CLI process did not report an exit status")
    }
}

impl Drop for CliSession {
    fn drop(&mut self) {
        let _ = self.session.close();
    }
}

/// Wait for a lifecycle artifact to disappear without assuming a cleanup delay.
pub fn wait_for_path_absent(path: &std::path::Path) -> Result<()> {
    poll_until(WAIT, POLL_INTERVAL, || (!path.exists()).then_some(()))
        .with_context(|| format!("path remained after process cleanup: {}", path.display()))
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.session.close();
            self.closed = true;
        }
    }
}

pub struct IpcCommand {
    child: Child,
    stdout: NamedTempFile,
    stderr: NamedTempFile,
}

/// Completed CLI IPC invocation, including expected non-zero responses.
pub struct IpcOutcome {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub json: Value,
}

impl IpcCommand {
    pub fn finish(self) -> Result<Value> {
        let outcome = self.finish_with_status()?;
        ensure!(
            outcome.status.success(),
            "IPC failed: {}; stdout={}; stderr={}",
            outcome.status,
            outcome.stdout,
            outcome.stderr
        );
        Ok(outcome.json)
    }

    /// Wait for the CLI and retain its parsed JSON even when it exits non-zero.
    ///
    /// The ordinary `finish` path remains success-only; this variant is for
    /// policy and approval responses whose structured error is expected.
    pub fn finish_with_status(mut self) -> Result<IpcOutcome> {
        try_poll_until(WAIT, POLL_INTERVAL, || {
            let Some(status) = self.child.try_wait()? else {
                return Ok(None);
            };
            let stdout = fs::read_to_string(self.stdout.path())?;
            let stderr = fs::read_to_string(self.stderr.path())?;
            let json = serde_json::from_str(&stdout).or_else(|stdout_error| {
                serde_json::from_str(&stderr).with_context(|| {
                    format!(
                        "invalid IPC JSON (stdout: {stdout_error}): stdout={stdout}; stderr={stderr}"
                    )
                })
            })?;
            Ok(Some(IpcOutcome {
                status,
                stdout,
                stderr,
                json,
            }))
        })?
        .context("IPC client timed out")
    }
}

impl Drop for IpcCommand {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn session_state(session: &Session) -> Result<State> {
    match session.execute(Operation::State)? {
        OperationResult::State(state) => Ok(*state),
        _ => bail!("unexpected state response"),
    }
}

/// Block until the session's process exits and return the state observed
/// afterwards.
fn wait_for_session_exit(session: &Session) -> Result<State> {
    session.execute(Operation::WaitExit {
        timeout_ms: Some(30_000),
    })?;
    session_state(session)
}

pub(super) fn output_events(recording: &str) -> Result<String> {
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
