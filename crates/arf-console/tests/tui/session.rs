use super::support::{DEFAULT_CONFIG, ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
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
            r"cat(paste0('RECOVERY_', 'OK'), '\n')",
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
                r"cat(paste0('RESIZED_', 'READY'), '\n')",
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

#[test]
fn startup_width_is_applied_to_r() -> Result<()> {
    run_case_with(
        Terminal::builder("startup-width")
            .args(["--no-auto-match"])
            .cols(120)
            .rows(32),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.submit("getOption('width')", "[1] 120", PROMPT)?;
            terminal.quit()
        },
    )
}

#[test]
fn auto_width_disabled_preserves_explicit_width_after_idle_boundary() -> Result<()> {
    run_case_with(
        Terminal::builder("fixed-width")
            .args([
                "--no-auto-match",
                "--with-ipc",
                "--ipc-eval-allow-function",
                "getOption",
            ])
            .config("[r]\nauto_width = false\n".to_owned() + DEFAULT_CONFIG)
            .cols(120)
            .rows(32),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter("options(width = 42)")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            // IPC evaluation is serviced after the REPL's idle callback. This
            // avoids racing a width synchronization against the next input.
            let response = terminal
                .start_ipc(&["eval", "getOption('width')", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["error"].is_null()
                    && response["value"]
                        .as_str()
                        .is_some_and(|value| value.trim() == "[1] 42"),
                "wrong R width after idle synchronization: {response}"
            );
            terminal.quit()
        },
    )
}

#[test]
fn error_handler_does_not_leak_variables_into_globalenv() -> Result<()> {
    run_case_with(
        Terminal::builder("error-globalenv")
            .args(["--no-auto-match"])
            .config("[startup]\nshow_banner = false\n".to_owned() + DEFAULT_CONFIG),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.submit(
                "stop('globalenv_leak_check')",
                "Error: globalenv_leak_check",
                ERROR_PROMPT,
            )?;
            terminal.submit("identical(ls(), character(0))", "[1] TRUE", PROMPT)?;
            terminal.quit()
        },
    )
}
