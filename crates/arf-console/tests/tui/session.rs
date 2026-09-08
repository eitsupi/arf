use super::support::{ERROR_PROMPT, PROMPT, run_case};
use anyhow::{Result, ensure};
use tui_test::Operation;

#[test]
fn startup_shows_banner_and_exits_cleanly() -> Result<()> {
    run_case("startup", &[], |terminal| {
        let text = terminal.state()?.text;
        ensure!(
            text.contains("# arf console v"),
            "missing arf banner: {text}"
        );
        ensure!(text.contains("is ready."), "missing R banner: {text}");
        Ok(())
    })
}

#[test]
fn evaluation_prints_values_and_preserves_variables() -> Result<()> {
    run_case("evaluation", &[], |terminal| {
        terminal.submit("x <- 41; x + 1", "[1] 42", PROMPT)?;
        terminal.submit("x + 2", "[1] 43", PROMPT)
    })
}

#[test]
fn evaluation_error_changes_status_then_next_command_recovers() -> Result<()> {
    run_case("error-recovery", &[], |terminal| {
        terminal.submit("stop('tui failure')", "Error: tui failure", ERROR_PROMPT)?;
        terminal.submit(
            "cat(paste0('RECOVERY_', 'OK'), '\\n')",
            "RECOVERY_OK",
            PROMPT,
        )
    })
}

#[test]
fn resize_updates_terminal_and_r_width() -> Result<()> {
    run_case(
        "resize",
        &["--with-ipc", "--ipc-eval-allow-function", "getOption"],
        |terminal| {
            terminal.execute(Operation::Resize {
                cols: 120,
                rows: 36,
            })?;
            // The REPL's idle callback synchronizes width before servicing IPC.
            // Query there instead of racing an Enter key against the resize event.
            let response = terminal
                .start_ipc(&["eval", "getOption('width')", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["error"].is_null()
                    && response["value"]
                        .as_str()
                        .is_some_and(|value| value.trim() == "[1] 120"),
                "wrong R width: {response}"
            );
            terminal.submit(
                "cat(paste0('RESIZED_', 'READY'), '\\n')",
                "RESIZED_READY",
                PROMPT,
            )?;
            let state = terminal.state()?;
            ensure!(
                (state.cols, state.rows) == (120, 36),
                "wrong size: {state:?}"
            );
            Ok(())
        },
    )
}
