use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case_with};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

fn history_status(history_dir: &Path, source: &str) -> Result<Option<i64>> {
    let path = history_dir.join("r.db");
    ensure!(
        path.is_file(),
        "history database is missing: {}",
        path.display()
    );
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection
        .query_row(
            "SELECT exit_status FROM history WHERE command_line = ?1 ORDER BY id DESC LIMIT 1",
            [source],
            |row| row.get(0),
        )
        .context("command missing from history")
}

#[test]
fn handled_errors_and_normal_conditions_are_successful_commands() -> Result<()> {
    let history = tempfile::tempdir()?;
    let commands = [
        (
            "tryCatch(stop('caught stop'), error = function(e) 'handled')",
            "[1] \"handled\"",
        ),
        ("signalCondition(simpleCondition('condition ping'))", ""),
        ("warning('ordinary warning')", "Warning message:"),
        ("message('ordinary message')", "ordinary message"),
        (
            "cat('ordinary stderr\\n', file = stderr())",
            "ordinary stderr",
        ),
    ];

    run_case_with(
        Terminal::builder("native-outcome-handled")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            for (source, marker) in commands {
                terminal.enter(source)?;
                terminal.wait_for("successful command prompt", |state, line| {
                    line.trim_end() == PROMPT && (marker.is_empty() || state.text.contains(marker))
                })?;
            }
            terminal.quit()
        },
    )?;

    for (source, _) in commands {
        ensure!(
            history_status(history.path(), source)? == Some(0),
            "handled condition or ordinary output should have success status: {source}"
        );
    }
    Ok(())
}

#[test]
fn expression_prefix_effects_survive_later_eval_parse_and_print_failures() -> Result<()> {
    const EVAL_FAILURE: &str =
        "globalCallingHandlers(NULL); outcome_eval_prefix <- 41; stop('late eval failure')";
    const PARSE_FAILURE: &str = "outcome_parse_prefix <- 42; 1 + * 2";
    const PRINT_FAILURE: &str = "structure(1, class = 'outcome_tui_failure')";

    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("native-outcome-phases")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;

            terminal.enter(EVAL_FAILURE)?;
            terminal.wait_for("evaluation failure outcome", |state, line| {
                state.text.contains("Error: late eval failure") && line.trim_end() == ERROR_PROMPT
            })?;
            terminal.submit("outcome_eval_prefix", "[1] 41", PROMPT)?;

            terminal.enter(PARSE_FAILURE)?;
            terminal.wait_for("parse failure after completed prefix", |state, line| {
                state.text.contains("Error") && line.trim_end() == ERROR_PROMPT
            })?;
            terminal.submit("outcome_parse_prefix", "[1] 42", PROMPT)?;

            terminal
                .enter("print.outcome_tui_failure <- function(x, ...) stop('autoprint failure')")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.enter(PRINT_FAILURE)?;
            terminal.wait_for("visible print failure outcome", |state, line| {
                state.text.contains("autoprint failure") && line.trim_end() == ERROR_PROMPT
            })?;
            terminal.submit("2 + 2", "[1] 4", PROMPT)?;
            terminal.enter("rm(print.outcome_tui_failure)")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.quit()
        },
    )?;

    for source in [EVAL_FAILURE, PARSE_FAILURE, PRINT_FAILURE] {
        ensure!(
            history_status(history.path(), source)? == Some(1),
            "failed command should have error status: {source}"
        );
    }
    ensure!(
        history_status(history.path(), "2 + 2")? == Some(0),
        "recovery command should have success status"
    );
    Ok(())
}
