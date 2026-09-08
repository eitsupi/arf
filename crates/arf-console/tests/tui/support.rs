//! Isolated real arf sessions; terminal transport and emulation belong to tui-test.

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
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

pub const DEFAULT_CONFIG: &str = r#"[prompt]
format = '{status}ARF> '
[prompt.status.symbol]
error = 'ERR '
"#;

/// Resources which can safely be shared by several terminal sessions.
///
/// In particular, the IPC server uses one directory for all sessions. A
/// session still gets its own working directory, config and history directory
/// so parallel cases cannot overwrite one another's files.
#[derive(Clone)]
pub struct SharedResources {
    root: Arc<TempDir>,
    sessions_dir: PathBuf,
}

impl SharedResources {
    pub fn new() -> Result<Self> {
        let root = Arc::new(tempfile::tempdir()?);
        let sessions_dir = root.path().join("sessions");
        fs::create_dir_all(&sessions_dir)?;
        Ok(Self { root, sessions_dir })
    }

    fn session_work(&self) -> Result<TempDir> {
        Ok(tempfile::tempdir_in(self.root.path())?)
    }

    fn sessions_dir(&self) -> &std::path::Path {
        &self.sessions_dir
    }
}

/// Configuration used to launch one real arf process.
pub struct TerminalBuilder {
    name: String,
    args: Vec<String>,
    config: Option<ConfigSource>,
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
    cols: u16,
    rows: u16,
    history_dir: Option<PathBuf>,
    artifacts: Option<PathBuf>,
    resources: Option<SharedResources>,
    vanilla: bool,
}

enum ConfigSource {
    Contents(String),
    Path(PathBuf),
}

impl TerminalBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
            config: None,
            env: Vec::new(),
            cwd: None,
            cols: 100,
            rows: 32,
            history_dir: None,
            artifacts: None,
            resources: None,
            vanilla: true,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Use TOML contents for the process-local config file.
    pub fn config(mut self, config: impl Into<String>) -> Self {
        self.config = Some(ConfigSource::Contents(config.into()));
        self
    }

    /// Use an existing config file without copying it into the session workdir.
    pub fn config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config = Some(ConfigSource::Path(path.into()));
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    pub fn cols(mut self, cols: u16) -> Self {
        self.cols = cols;
        self
    }

    pub fn rows(mut self, rows: u16) -> Self {
        self.rows = rows;
        self
    }

    pub fn history_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.history_dir = Some(path.into());
        self
    }

    pub fn artifacts(mut self, path: impl Into<PathBuf>) -> Self {
        self.artifacts = Some(path.into());
        self
    }

    pub fn resources(mut self, resources: SharedResources) -> Self {
        self.resources = Some(resources);
        self
    }

    /// Keep the default isolated startup flags unless a startup profile must
    /// be exercised by a test.
    pub fn vanilla(mut self, enabled: bool) -> Self {
        self.vanilla = enabled;
        self
    }

    fn ensure_artifacts(&mut self) -> Result<PathBuf> {
        if let Some(path) = &self.artifacts {
            fs::create_dir_all(path)?;
            return Ok(path.clone());
        }
        let root = std::env::var_os("ARF_TUI_TEST_ARTIFACTS")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&root)?;
        let path = tempfile::Builder::new()
            .prefix(&format!("arf-tui-{}-", self.name))
            .tempdir_in(root)?
            .keep();
        self.artifacts = Some(path.clone());
        Ok(path)
    }

    /// Spawn arf and return before waiting for its first prompt.
    pub fn spawn(mut self) -> Result<Terminal> {
        let cols = self.cols;
        let rows = self.rows;
        let artifacts = self.ensure_artifacts()?;
        let resources = match self.resources {
            Some(resources) => resources,
            None => SharedResources::new()?,
        };
        let work = resources.session_work()?;
        let config = match self.config {
            Some(ConfigSource::Contents(contents)) => {
                let path = work.path().join("config.toml");
                fs::write(&path, contents)?;
                path
            }
            Some(ConfigSource::Path(path)) => {
                if path.is_absolute() {
                    path
                } else {
                    // Resolve before launching the child: its cwd may be a
                    // session-specific temporary directory.
                    std::env::current_dir()?.join(path)
                }
            }
            None => {
                let path = work.path().join("config.toml");
                fs::write(&path, DEFAULT_CONFIG)?;
                path
            }
        };
        let history_dir = self
            .history_dir
            .unwrap_or_else(|| work.path().join("history"));
        let defaults = OpenOptions::default();
        let mut args = vec!["--no-r-source-overrides".to_owned()];
        if self.vanilla {
            args.insert(0, "--vanilla".to_owned());
        }
        args.extend([
            "--config".to_owned(),
            config.to_string_lossy().into_owned(),
            "--history-dir".to_owned(),
            history_dir.to_string_lossy().into_owned(),
        ]);
        args.extend(self.args);
        let cwd = self.cwd.unwrap_or_else(|| work.path().to_path_buf());
        let mut env = self.env;
        if let Some((_, value)) = env
            .iter_mut()
            .find(|(key, _)| key == "ARF_IPC_SESSIONS_DIR")
        {
            *value = resources.sessions_dir().to_string_lossy().into_owned();
        } else {
            env.push((
                "ARF_IPC_SESSIONS_DIR".into(),
                resources.sessions_dir().to_string_lossy().into_owned(),
            ));
        }
        let mut terminal = Terminal {
            session: Session::new(&self.name),
            artifacts,
            work,
            resources,
            cwd: cwd.clone(),
            pid: None,
            closed: false,
        };
        terminal.stage("spawn")?;
        let opened = terminal.session.run(RunOptions {
            backend: defaults.backend,
            program: env!("CARGO_BIN_EXE_arf").into(),
            args,
            profile: defaults.profile,
            cols,
            rows,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            env,
            wait_ready: Some(false),
            restart: false,
            timeouts: defaults.timeouts,
            recording: AutomaticRecording {
                mode: AutomaticRecordingMode::Always,
                directory: Some(terminal.artifacts.clone()),
            },
        })?;
        terminal.pid = opened.shell_pid;
        Ok(terminal)
    }
}

pub struct Terminal {
    session: Session,
    artifacts: PathBuf,
    work: TempDir,
    resources: SharedResources,
    cwd: PathBuf,
    pid: Option<u32>,
    closed: bool,
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

    /// Poll a fixed emulated screen row, which is useful for pager headers
    /// rendered above the hidden cursor.
    pub fn wait_for_screen_line(
        &self,
        description: &str,
        y: u16,
        predicate: impl Fn(&State, &str) -> bool,
    ) -> Result<()> {
        self.stage(description)?;
        let started = Instant::now();
        loop {
            let state = self.state()?;
            let line = self.screen_line(y, state.cols)?;
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

    fn ipc_sessions_dir(&self) -> &std::path::Path {
        self.resources.sessions_dir()
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
            .env("ARF_IPC_SESSIONS_DIR", self.ipc_sessions_dir())
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
        CliSession::new("arf-cli-tty", args, &self.cwd, self.ipc_sessions_dir())
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
        self.execute(Operation::WaitExit {
            timeout_ms: Some(30_000),
        })?;
        let state = self.state()?;
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
        match self.session.execute(Operation::State)? {
            OperationResult::State(state) => Ok(state),
            _ => bail!("unexpected CLI state response"),
        }
    }

    pub fn wait_for_exit(&self) -> Result<i32> {
        self.session.execute(Operation::WaitExit {
            timeout_ms: Some(30_000),
        })?;
        self.state()?
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
    let started = Instant::now();
    while path.exists() {
        ensure!(
            started.elapsed() < WAIT,
            "path remained after process cleanup: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
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
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                let stdout = fs::read_to_string(self.stdout.path())?;
                let stderr = fs::read_to_string(self.stderr.path())?;
                let json = serde_json::from_str(&stdout).or_else(|stdout_error| {
                    serde_json::from_str(&stderr).with_context(|| {
                        format!(
                            "invalid IPC JSON (stdout: {stdout_error}): stdout={stdout}; stderr={stderr}"
                        )
                    })
                })?;
                return Ok(IpcOutcome {
                    status,
                    stdout,
                    stderr,
                    json,
                });
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
    let builder = Terminal::builder(name).args(args.iter().copied());
    run_case_with(builder, |terminal| {
        terminal.wait_for_first_prompt()?;
        test(terminal)?;
        terminal.quit()
    })
}

/// Run a terminal case with the common watchdog, diagnostics and cleanup.
///
/// The callback runs immediately after spawning. This deliberately leaves
/// readiness and process exit policy to the caller: migration tests may need
/// to inspect startup output, send a signal, or observe an already-exited
/// process instead of unconditionally submitting `q('no')`.
pub fn run_case_with(
    mut builder: TerminalBuilder,
    test: impl FnOnce(&Terminal) -> Result<()>,
) -> Result<()> {
    let name = builder.name.clone();
    let artifacts = builder.ensure_artifacts()?;
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
    let mut terminal = None;
    let result = catch_unwind(AssertUnwindSafe(|| {
        terminal = Some(builder.spawn()?);
        let terminal_ref = terminal.as_ref().expect("terminal was just spawned");
        test(terminal_ref)
    }));
    if let Some(terminal_ref) = terminal.as_ref() {
        match &result {
            Ok(Err(error)) => {
                let _ = fs::write(
                    terminal_ref.artifacts.join("failure.txt"),
                    format!("{error:#}"),
                );
            }
            Err(_) => {
                let _ = fs::write(
                    terminal_ref.artifacts.join("failure.txt"),
                    "test panicked; see cargo test output",
                );
            }
            _ => {}
        }
    } else if let Ok(Err(error)) = &result {
        let _ = fs::write(artifacts.join("failure.txt"), format!("{error:#}"));
    }
    // Always close an opened session, including when the test body returned an
    // error or panicked. Session's own Drop is a final backstop for failures
    // before this point (for example, a spawn error).
    let close_stage = terminal
        .as_ref()
        .map(|terminal_ref| terminal_ref.stage("close"));
    let close = terminal
        .as_ref()
        .map(|terminal_ref| terminal_ref.session.close());
    if close.as_ref().is_some_and(Result::is_ok) {
        let terminal_ref = terminal.as_mut().expect("close result has a terminal");
        terminal_ref.closed = true;
    }
    let result = match result {
        Ok(result) => result,
        Err(panic) => resume_unwind(panic),
    };
    let diagnostics = terminal
        .as_ref()
        .map(|terminal_ref| terminal_ref.artifacts.display().to_string())
        .unwrap_or_else(|| artifacts.display().to_string());
    result.with_context(|| format!("{name}; diagnostics: {diagnostics}"))?;
    if let Some(close) = close {
        close.context("close PTY")?;
    }
    if let Some(close_stage) = close_stage {
        close_stage?;
    }
    if let Some(terminal_ref) = terminal.as_ref() {
        terminal_ref.stage("passed")?;
    }
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

#[test]
fn builder_applies_process_settings_and_shares_ipc_resources() -> Result<()> {
    let cwd = tempfile::tempdir()?;
    let history = tempfile::tempdir()?;
    let artifacts = tempfile::tempdir()?;
    let second_artifacts = tempfile::tempdir()?;
    let current_dir = std::env::current_dir()?;
    let config_file = tempfile::Builder::new()
        .prefix("arf-tui-config-")
        .tempfile_in(&current_dir)?;
    fs::write(
        config_file.path(),
        "[startup]\nshow_banner = false\n[prompt]\nformat = 'PATH> '",
    )?;
    let config_relative = config_file
        .path()
        .file_name()
        .context("temporary config has no file name")?
        .to_owned();
    let resources = SharedResources::new()?;
    let cwd_text = cwd.path().to_string_lossy().replace('\\', "/");
    let shared_sessions = resources.sessions_dir().to_path_buf();

    run_case_with(
        Terminal::builder("builder-settings")
            .config("[startup]\nshow_banner = false\n".to_owned() + DEFAULT_CONFIG)
            .env("ARF_BUILDER_ENV", "configured")
            .cwd(cwd.path())
            .cols(90)
            .rows(20)
            .history_dir(history.path())
            .artifacts(artifacts.path())
            .resources(resources.clone()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let state = terminal.state()?;
            ensure!(
                (state.cols, state.rows) == (90, 20),
                "wrong size: {state:?}"
            );
            ensure!(terminal.pid().is_some(), "missing arf PID");
            ensure!(
                terminal.sessions_dir() == shared_sessions,
                "wrong shared IPC directory"
            );
            ensure!(
                !state.text.contains("# arf console v"),
                "custom config did not disable the banner"
            );
            terminal.submit(
                "Sys.getenv('ARF_BUILDER_ENV')",
                r#"[1] "configured""#,
                PROMPT,
            )?;
            terminal.submit(
                &format!(
                    "normalizePath(getwd(), winslash='/') == normalizePath('{cwd_text}', winslash='/')"
                ),
                "[1] TRUE",
                PROMPT,
            )?;
            terminal.quit()
        },
    )?;
    ensure!(
        fs::read_dir(artifacts.path())?.next().is_some(),
        "recording artifacts were not retained"
    );
    ensure!(
        history.path().join("r.db").is_file(),
        "custom history directory did not contain r.db"
    );

    run_case_with(
        Terminal::builder("builder-shared-resource")
            .config_path(config_relative)
            .artifacts(second_artifacts.path())
            .resources(resources),
        |terminal| {
            terminal.wait_for_prompt(None, "PATH>")?;
            ensure!(
                terminal.sessions_dir() == shared_sessions,
                "second session did not reuse shared IPC directory"
            );
            terminal.quit()
        },
    )
}
