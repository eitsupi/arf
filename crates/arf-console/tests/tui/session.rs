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
fn native_error_detection_does_not_add_globalenv_bindings() -> Result<()> {
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
            terminal.submit(
                r#"length(grep("^\\.arf_", ls(all.names = TRUE)))"#,
                "[1] 0",
                PROMPT,
            )?;
            terminal.quit()
        },
    )
}

#[test]
fn startup_error_option_survives_native_driver_initialization() -> Result<()> {
    let profile = tempfile::tempdir()?;
    let profile_path = profile.path().join("profile.R");
    std::fs::write(
        &profile_path,
        r#"
stopifnot(is.null(getOption("error")))
startup_handler <- function() invisible(NULL)
options(error = startup_handler)
stopifnot(is.call(getOption("error")), is.function(getOption("error")[[1L]]))
startup_expression <- expression(invisible(NULL))
options(error = startup_expression)
stopifnot(is.expression(getOption("error")))
options(error = utils::recover)
stopifnot(is.call(getOption("error")), is.function(getOption("error")[[1L]]))
cat("STARTUP_ERROR_OPTION_PRESERVED\n")
"#,
    )?;

    run_case_with(
        Terminal::builder("startup-error-option")
            .args(["--no-auto-match"])
            .vanilla(false)
            .env(
                "R_PROFILE_USER",
                profile_path.to_string_lossy().into_owned(),
            ),
        |terminal| {
            terminal.wait_for("startup error option profile", |state, _| {
                state.text.contains("STARTUP_ERROR_OPTION_PRESERVED")
            })?;
            terminal.wait_for_first_prompt()?;
            terminal.submit(
                "is.call(getOption('error')) && is.function(getOption('error')[[1L]])",
                "[1] TRUE",
                PROMPT,
            )?;
            terminal.enter("options(error = NULL)")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.quit()
        },
    )
}

#[test]
fn runtime_error_options_are_preserved_and_native_outcomes_recover() -> Result<()> {
    run_case_with(
        Terminal::builder("runtime-error-options")
            .args(["--no-auto-match"])
            .config("[startup]\nshow_banner = false\n".to_owned() + DEFAULT_CONFIG),
        |terminal| {
            terminal.wait_for_first_prompt()?;

            terminal.submit(
                "options(error = NULL); stop('null error handler')",
                "Error: null error handler",
                ERROR_PROMPT,
            )?;
            terminal.submit("is.null(getOption('error'))", "[1] TRUE", PROMPT)?;

            terminal.enter(
                "native_function_handler <- function() assign('native_function_handler_called', TRUE, envir = .GlobalEnv)",
            )?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter("options(error = native_function_handler)")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.submit(
                "stop('function error handler')",
                "Error: function error handler",
                ERROR_PROMPT,
            )?;
            terminal.submit(
                "is.call(getOption('error')) && is.function(getOption('error')[[1L]]) && isTRUE(native_function_handler_called)",
                "[1] TRUE",
                PROMPT,
            )?;

            terminal.submit(
                "options(error = expression(assign('native_expression_handler_called', TRUE, envir = .GlobalEnv))); stop('expression error handler')",
                "Error: expression error handler",
                ERROR_PROMPT,
            )?;
            terminal.submit(
                "is.expression(getOption('error')) && isTRUE(native_expression_handler_called)",
                "[1] TRUE",
                PROMPT,
            )?;

            terminal.submit(
                "options(error = utils::recover); is.call(getOption('error')) && is.function(getOption('error')[[1L]])",
                "[1] TRUE",
                PROMPT,
            )?;
            terminal.enter("options(error = NULL)")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.quit()
        },
    )
}

#[test]
fn native_error_detection_survives_globalenv_clear_and_workspace_reload() -> Result<()> {
    let workdir = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("error-workspace-reload")
            .args(["--no-auto-match"])
            .cwd(workdir.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.submit(
                r#"length(grep("^\\.arf_", ls(.GlobalEnv, all.names = TRUE)))"#,
                "[1] 0",
                PROMPT,
            )?;
            terminal.enter(
                "local({ assign('.arf_saved_detection_sentinel', TRUE, envir = .GlobalEnv); save.image('.RData') })",
            )?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter(
                "local({ rm(list = ls(all.names = TRUE), envir = .GlobalEnv); load('.RData', envir = .GlobalEnv) })",
            )?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.submit(
                "isTRUE(.arf_saved_detection_sentinel) && length(grep('^\\\\.arf_', ls(all.names = TRUE))) == 1",
                "[1] TRUE",
                PROMPT,
            )?;
            terminal.submit(
                "stop('error after workspace reload')",
                "Error: error after workspace reload",
                ERROR_PROMPT,
            )?;
            terminal.submit("2 + 2", "[1] 4", PROMPT)
        },
    )
}
