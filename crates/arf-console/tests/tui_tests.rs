//! Real interactive arf smoke test on Unix PTYs and Windows ConPTY.
//!
//! Keep the existing Unix PTY regression suite while evaluating tui-test's
//! Alacritty-backed terminal. No shell, CLI daemon, or optional backend is needed.

use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tui_test::{
    AutomaticRecording, AutomaticRecordingMode, KeyAction, OpenOptions, Operation, OperationResult,
    RunOptions, Session, State,
};

const WAIT: Duration = Duration::from_secs(30);
const PROMPT: &str = "ARF>";
const ERROR_PROMPT: &str = "ERR ARF>";

struct Terminal {
    session: Session,
    artifacts: PathBuf,
}

impl Terminal {
    fn stage(&self, name: &str) -> Result<()> {
        eprintln!("tui-test: {name} (artifacts: {})", self.artifacts.display());
        fs::write(self.artifacts.join("stage.txt"), name)?;
        Ok(())
    }

    fn execute(&self, operation: Operation) -> Result<OperationResult> {
        Ok(self.session.execute(operation)?)
    }

    fn state(&self) -> Result<State> {
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

    fn wait_for(&self, description: &str, predicate: impl Fn(&State, &str) -> bool) -> Result<()> {
        let started = Instant::now();
        loop {
            let state = self.state()?;
            let line = self.current_line(&state)?;
            // Persist the latest observation even if a subsequent library call hangs.
            fs::write(
                self.artifacts.join("state.json"),
                serde_json::to_vec_pretty(&state)?,
            )?;
            if predicate(&state, &line) {
                return Ok(());
            }
            ensure!(
                state.exited.is_none(),
                "arf exited while waiting for {description}: {state:?}"
            );
            ensure!(
                started.elapsed() < WAIT,
                "timed out waiting for {description}: {state:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_prompt(&self, output: Option<&str>, prompt: &str) -> Result<()> {
        // Check the cursor's line, not an old prompt in scrollback. Output markers
        // must occupy an entire line and differ from the echoed R source.
        // WaitReady/WaitCommand rely on shell integration, which arf does not emit.
        // Read State.text/cells directly: beta.3's text locator loses wide and
        // combining characters when flattening the terminal grid.
        self.wait_for("new output and input prompt", |state, line| {
            line.trim_end() == prompt
                && usize::from(state.cursor.x) == prompt.len() + 1
                && output.is_none_or(|text| state.text.lines().any(|line| line == text))
        })
    }

    fn submit(&self, source: &str, output: &str, prompt: &str) -> Result<()> {
        self.execute(Operation::Submit {
            data: Some(source.into()),
        })?;
        self.wait_for_prompt(Some(output), prompt)
    }

    fn key(&self, key: &str) -> Result<()> {
        self.execute(Operation::Key {
            keys: vec![key.into()],
            action: KeyAction::Press,
        })?;
        Ok(())
    }

    fn smoke(&self, work: &Path) -> Result<()> {
        let config = work.join("config.toml");
        fs::write(
            &config,
            "[prompt]\nformat = '{status}ARF> '\n[prompt.status.symbol]\nerror = 'ERR '\n",
        )?;
        let defaults = OpenOptions::default();
        self.stage("spawn")?;
        self.session.run(RunOptions {
            backend: defaults.backend,
            program: env!("CARGO_BIN_EXE_arf").into(),
            args: vec![
                "--vanilla".into(),
                "--no-history".into(),
                "--no-r-source-overrides".into(),
                "--config".into(),
                config.to_string_lossy().into_owned(),
            ],
            profile: defaults.profile,
            cols: 100,
            rows: 32,
            cwd: Some(work.to_string_lossy().into_owned()),
            env: vec![(
                "ARF_IPC_SESSIONS_DIR".into(),
                work.join("sessions").to_string_lossy().into_owned(),
            )],
            wait_ready: Some(false),
            restart: false,
            timeouts: defaults.timeouts,
            recording: AutomaticRecording {
                mode: AutomaticRecordingMode::Always,
                directory: Some(self.artifacts.clone()),
            },
        })?;
        self.stage("startup prompt")?;
        self.wait_for_prompt(None, PROMPT)?;

        self.stage("evaluation")?;
        self.submit("1 + 41", "[1] 42", PROMPT)?;
        self.stage("error and recovery")?;
        self.submit("stop('tui failure')", "Error: tui failure", ERROR_PROMPT)?;
        self.submit(
            "cat(paste0('RECOVERY_', 'OK'), '\\n')",
            "RECOVERY_OK",
            PROMPT,
        )?;

        self.stage("editing keys")?;
        self.execute(Operation::Write {
            data: "1 + 40".into(),
        })?;
        self.wait_for("input echo", |_, line| line.trim_end() == "ARF> 1 + 40")?;
        self.key("Left")?;
        self.key("Backspace")?;
        self.execute(Operation::Write { data: "2".into() })?;
        self.wait_for("edited input", |_, line| line.trim_end() == "ARF> 1 + 20")?;
        self.execute(Operation::Submit { data: None })?;
        self.wait_for_prompt(Some("[1] 21"), PROMPT)?;

        self.stage("Ctrl+C cancels input")?;
        self.execute(Operation::Write {
            data: "unfinished_input".into(),
        })?;
        self.wait_for("input before cancellation", |_, line| {
            line.trim_end() == "ARF> unfinished_input"
        })?;
        self.key("Ctrl+C")?;
        self.wait_for_prompt(None, PROMPT)?;
        self.submit(
            "Sys.sleep(0.4); cat(paste0('CANCEL_', 'OK'), '\\n')",
            "CANCEL_OK",
            PROMPT,
        )?;

        self.stage("Unicode output")?;
        self.submit(
            "cat(intToUtf8(c(26085, 26412, 35486)), '\\n')",
            "日本語",
            PROMPT,
        )?;
        self.stage("resize and subsequent evaluation")?;
        self.execute(Operation::Resize {
            cols: 120,
            rows: 36,
        })?;
        self.submit("cat(paste0('RESIZED_', 'OK'), '\\n')", "RESIZED_OK", PROMPT)?;
        let state = self.state()?;
        ensure!(
            (state.cols, state.rows) == (120, 36),
            "wrong size: {state:?}"
        );

        self.stage("normal exit")?;
        self.execute(Operation::Submit {
            data: Some("q('no')".into()),
        })?;
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

#[test]
fn interactive_smoke() -> Result<()> {
    let root = std::env::var_os("ARF_TUI_TEST_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root)?;
    let artifacts = tempfile::Builder::new()
        .prefix("arf-tui-")
        .tempdir_in(root)?
        .keep();
    // A library wait/close can block in platform PTY code outside its own timeout.
    // Keep this independent watchdog alive through Session drop, including unwind.
    // Exiting this dedicated test binary also releases its PTY handles.
    let (_watchdog, deadline) = mpsc::channel::<()>();
    let diagnostics = artifacts.clone();
    thread::spawn(move || {
        if matches!(
            deadline.recv_timeout(Duration::from_secs(180)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            eprintln!(
                "tui-test exceeded 180s; diagnostics: {}",
                diagnostics.display()
            );
            std::process::exit(1);
        }
    });
    let work = tempfile::tempdir()?;
    let terminal = Terminal {
        session: Session::new("arf-smoke"),
        artifacts,
    };
    let result = terminal.smoke(work.path());
    if let Err(error) = &result {
        eprintln!("tui-test failed: {error:#}");
        fs::write(terminal.artifacts.join("failure.txt"), format!("{error:#}"))?;
    }
    terminal.stage("close")?;
    let close = terminal.session.close();
    result.context("interactive smoke test")?;
    close.context("close PTY")?;
    terminal.stage("passed")?;
    Ok(())
}
