use super::builder::TerminalBuilder;
use super::terminal::Terminal;
use crate::common::spawn_watchdog;
use anyhow::{Context, Result};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::time::Duration;

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
    let diagnostics = artifacts.clone();
    let _watchdog = spawn_watchdog(Duration::from_secs(180), move || {
        let message = format!(
            "tui-test exceeded 180s; diagnostics: {}\n",
            diagnostics.display()
        );
        let _ = fs::write(diagnostics.join("timeout.txt"), &message);
        message
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
