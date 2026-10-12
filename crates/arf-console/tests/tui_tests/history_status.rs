use super::support::{DEFAULT_CONFIG, ERROR_PROMPT, PROMPT, Terminal, run_case_with};
use crate::common::read_history_rows;
use anyhow::{Context, Result, ensure};

#[test]
fn history_records_success_and_error_exit_status() -> Result<()> {
    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("history-exit-status")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.submit(
                "stop('history_exit_error')",
                "Error: history_exit_error",
                ERROR_PROMPT,
            )?;
            terminal.submit("1", "[1] 1", PROMPT)?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    let success = rows
        .iter()
        .find(|row| row.command == "42")
        .context("successful command missing from history")?;
    ensure!(
        success.exit_status == Some(0),
        "successful command has wrong status: {success:?}"
    );
    let error = rows
        .iter()
        .find(|row| row.command.contains("history_exit_error"))
        .context("error command missing from history")?;
    ensure!(
        error.exit_status == Some(1),
        "error command has wrong status: {error:?}"
    );
    Ok(())
}

#[test]
fn replacing_error_option_leaves_history_status_unavailable() -> Result<()> {
    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("history-error-option-unavailable")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.submit(
                "stop('known_before_handler_change')",
                "Error: known_before_handler_change",
                ERROR_PROMPT,
            )?;
            terminal.enter("options(error = function() stop('replacement_handler_error'))")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter("stop('unhandled_with_replacement')")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter("options(error = NULL)")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter("stop('unhandled_after_error_option_removal')")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    let initial_success = rows
        .iter()
        .find(|row| row.command == "42")
        .context("initial successful command missing from history")?;
    ensure!(
        initial_success.exit_status == Some(0),
        "wrapper should record success before the handler is changed: {initial_success:?}"
    );
    let known_error = rows
        .iter()
        .find(|row| row.command == "stop('known_before_handler_change')")
        .context("known error command missing from history")?;
    ensure!(
        known_error.exit_status == Some(1),
        "wrapper should record failure before the handler is changed: {known_error:?}"
    );
    for command in [
        "options(error = function() stop('replacement_handler_error'))",
        "stop('unhandled_with_replacement')",
        "options(error = NULL)",
        "stop('unhandled_after_error_option_removal')",
    ] {
        let row = rows
            .iter()
            .find(|row| row.command == command)
            .with_context(|| format!("history entry missing for {command}"))?;
        ensure!(
            row.exit_status.is_none(),
            "{command} should have unknown status: {row:?}"
        );
    }
    Ok(())
}

#[test]
fn locked_error_flag_makes_outcome_unavailable_without_stopping_repl() -> Result<()> {
    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("history-locked-error-flag")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter("lockBinding('had_error', get('.arf_error_state', .GlobalEnv)); 42")?;
            terminal.wait_for_prompt(Some("[1] 42"), PROMPT)?;
            terminal.enter("43")?;
            terminal.wait_for_prompt(Some("[1] 43"), PROMPT)?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    for value in ["42", "43"] {
        let row = rows
            .iter()
            .find(|row| row.command.contains(value))
            .with_context(|| format!("command {value} missing from history"))?;
        ensure!(
            row.exit_status.is_none(),
            "locked error state should produce unavailable outcome: {row:?}"
        );
    }
    Ok(())
}

#[test]
fn removed_error_state_chains_captured_handler_and_keeps_outcomes_unknown() -> Result<()> {
    let history = tempfile::tempdir()?;
    let profile = tempfile::tempdir()?;
    let profile_path = profile.path().join(".Rprofile");
    std::fs::write(
        &profile_path,
        "options(error = function() cat('captured_previous_handler_ran\\n'))\n",
    )?;
    let config = r#"
[experimental.history_forget]
enabled = true
delay = 0
on_exit_only = false
"#
    .to_owned()
        + DEFAULT_CONFIG;
    run_case_with(
        Terminal::builder("history-error-state-removed")
            .args(["--no-auto-match"])
            .vanilla(false)
            .env(
                "R_PROFILE_USER",
                profile_path.to_string_lossy().into_owned(),
            )
            .config(config)
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal
                .enter("rm('.arf_error_state', envir = .GlobalEnv); stop('untracked_error')")?;
            terminal.wait_for("captured previous error handler", |state, _| {
                state.text.contains("captured_previous_handler_ran")
            })?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    let untracked_error = rows
        .iter()
        .find(|row| row.command.contains("untracked_error"))
        .context("untracked failed command missing from history")?;
    ensure!(
        untracked_error.exit_status.is_none(),
        "error after tracking removal should have unknown status: {untracked_error:?}"
    );
    ensure!(
        rows.iter()
            .any(|row| row.command.contains("untracked_error")),
        "unavailable command should stay in history instead of being forgotten"
    );
    Ok(())
}

#[test]
fn active_error_state_binding_is_not_evaluated() -> Result<()> {
    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("history-active-error-state")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter(
                "rm('.arf_error_state', envir = .GlobalEnv); makeActiveBinding('.arf_error_state', function(value) { cat(paste0('ACTIVE', '_STATE_CALLED\\n')); stop('active_state_error') }, .GlobalEnv)",
            )?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter("42")?;
            terminal.wait_for(
                "REPL continues without invoking active binding",
                |state, _| {
                    state.text.contains("[1] 42") && !state.text.contains("ACTIVE_STATE_CALLED")
                },
            )?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    let active_binding_setup = rows
        .iter()
        .find(|row| row.command.contains("makeActiveBinding"))
        .context("active-binding command missing from history")?;
    ensure!(
        active_binding_setup.exit_status.is_none(),
        "active state binding should make the command outcome unavailable: {active_binding_setup:?}"
    );
    ensure!(
        rows.iter()
            .all(|row| !row.command.contains("ACTIVE_STATE_CALLED")),
        "active binding output must not be saved as user input"
    );
    Ok(())
}

#[test]
fn active_error_flag_binding_is_not_evaluated_by_tracking_or_handler() -> Result<()> {
    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("history-active-error-flag")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter(
                "state <- get('.arf_error_state', .GlobalEnv); rm('had_error', envir = state); makeActiveBinding('had_error', function(value) { cat(paste0('ACTIVE_', 'HAD_ERROR_CALLED\\n')); stop('active_had_error') }, state)",
            )?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter("stop('active_binding_trigger')")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.wait_for("active binding was not invoked", |state, _| {
                state.text.contains("Error: active_binding_trigger")
                    && !state.text.contains("ACTIVE_HAD_ERROR_CALLED")
            })?;
            terminal.enter("42")?;
            terminal.wait_for(
                "REPL recovers after unhandled error with active state binding",
                |state, _| {
                    state.text.contains("[1] 42") && !state.text.contains("ACTIVE_HAD_ERROR_CALLED")
                },
            )?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    for fragment in [
        "makeActiveBinding('had_error'",
        "stop('active_binding_trigger')",
        "42",
    ] {
        let row = rows
            .iter()
            .find(|row| row.command.contains(fragment))
            .with_context(|| format!("command containing {fragment:?} missing from history"))?;
        ensure!(
            row.exit_status.is_none(),
            "active had_error binding should make command outcome unavailable: {row:?}"
        );
    }
    Ok(())
}

#[test]
fn chained_user_error_handler_failure_is_still_recorded() -> Result<()> {
    let history = tempfile::tempdir()?;
    let profile = tempfile::tempdir()?;
    let profile_path = profile.path().join(".Rprofile");
    std::fs::write(
        &profile_path,
        "options(error = function() stop('nested_handler_error'))\n",
    )?;
    run_case_with(
        Terminal::builder("history-chained-error-handler")
            .args(["--no-auto-match"])
            .vanilla(false)
            .env(
                "R_PROFILE_USER",
                profile_path.to_string_lossy().into_owned(),
            )
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter("stop('wrapper_marks_before_chaining')")?;
            terminal.wait_for_prompt(None, ERROR_PROMPT)?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    let failed = rows
        .iter()
        .find(|row| row.command.contains("wrapper_marks_before_chaining"))
        .context("chained-handler command missing from history")?;
    ensure!(
        failed.exit_status == Some(1),
        "chained handler error should remain a known failure: {failed:?}"
    );
    Ok(())
}

#[test]
fn history_forget_delay_one_removes_old_errors_and_keeps_success() -> Result<()> {
    let history = tempfile::tempdir()?;
    let config = r#"
[experimental.history_forget]
enabled = true
delay = 1
on_exit_only = false
"#
    .to_owned()
        + DEFAULT_CONFIG;
    run_case_with(
        Terminal::builder("history-forget")
            .args(["--no-auto-match"])
            .config(config)
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.submit(
                "stop('history_forget_error_1')",
                "Error: history_forget_error_1",
                ERROR_PROMPT,
            )?;
            terminal.submit(
                "stop('history_forget_error_2')",
                "Error: history_forget_error_2",
                ERROR_PROMPT,
            )?;
            terminal.submit(
                "stop('history_forget_error_3')",
                "Error: history_forget_error_3",
                ERROR_PROMPT,
            )?;
            terminal.quit()
        },
    )?;

    let rows = read_history_rows(&history.path().join("r.db"))?;
    let commands: Vec<_> = rows.iter().map(|row| row.command.as_str()).collect();
    ensure!(commands.contains(&"42"), "success was forgotten");
    ensure!(
        commands
            .iter()
            .all(|command| !command.contains("history_forget_error_1")),
        "oldest failed command was not forgotten: {commands:?}"
    );
    ensure!(
        commands
            .iter()
            .all(|command| !command.contains("history_forget_error_2")),
        "second failed command was not forgotten: {commands:?}"
    );
    let failed: Vec<_> = commands
        .iter()
        .filter(|command| command.contains("history_forget_error_"))
        .collect();
    ensure!(
        failed.len() <= 1,
        "at most one failed command should remain: {failed:?}"
    );
    if let Some(command) = failed.first() {
        ensure!(
            command.contains("history_forget_error_3"),
            "remaining failed command should be the latest one: {command}"
        );
    }
    Ok(())
}
