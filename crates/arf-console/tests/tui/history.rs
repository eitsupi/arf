use super::support::{DEFAULT_CONFIG, ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

fn history_rows(history_dir: &Path) -> Result<Vec<(String, Option<i64>)>> {
    let path = history_dir.join("r.db");
    ensure!(
        path.is_file(),
        "history database is missing: {}",
        path.display()
    );
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement =
        connection.prepare("SELECT command_line, exit_status FROM history ORDER BY id")?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[test]
fn history_browser_reopens_after_writing_more_history() -> Result<()> {
    run_case(
        "history-browser",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.submit("1 + 1", "[1] 2", PROMPT)?;
            terminal.submit("print('hello')", r#"[1] "hello""#, PROMPT)?;
            terminal.enter(":history browse")?;
            terminal.wait_for("first history browser", |state, _| {
                state.text.contains("q exit") && state.text.contains("print('hello')")
            })?;
            terminal.key("q")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.enter(":history browse")?;
            terminal.wait_for("reopened history browser", |state, _| {
                state.text.contains("q exit") && state.text.contains("42")
            })?;
            terminal.key("q")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.submit("99", "[1] 99", PROMPT)
        },
    )
}

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

    let rows = history_rows(history.path())?;
    let success = rows
        .iter()
        .find(|(command, _)| command == "42")
        .context("successful command missing from history")?;
    ensure!(
        success.1 == Some(0),
        "successful command has wrong status: {success:?}"
    );
    let error = rows
        .iter()
        .find(|(command, _)| command.contains("history_exit_error"))
        .context("error command missing from history")?;
    ensure!(
        error.1 == Some(1),
        "error command has wrong status: {error:?}"
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

    let rows = history_rows(history.path())?;
    let commands: Vec<_> = rows.iter().map(|(command, _)| command.as_str()).collect();
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

#[test]
fn history_menu_selection_replaces_existing_buffer() -> Result<()> {
    let command = "history_menu_replace_value <- 999";
    run_case(
        "history-menu-replace",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.enter(command)?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.write("history_menu")?;
            terminal.wait_for("partial history input", |_, line| {
                line.trim_end().starts_with("ARF> history_menu")
            })?;
            terminal.write("\x12")?;
            terminal.wait_for("history item is visible", |state, _| {
                state.text.contains("Page 1:") && state.text.contains(command)
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("history item replaces buffer", |_, line| {
                line.trim_end() == format!("ARF> {command}")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.submit("history_menu_replace_value", "[1] 999", PROMPT)
        },
    )
}

#[test]
fn history_menu_selection_replaces_auto_matched_quote_pair() -> Result<()> {
    run_case(
        "history-menu-quote-pair",
        &["--no-completion"],
        |terminal| {
            terminal.submit(r#"nchar("``")"#, "[1] 2", PROMPT)?;
            let checkpoint = terminal.checkpoint()?;
            terminal.write("`")?;
            terminal.wait_for("auto-matched quote pair", |_, line| line.contains("``"))?;
            terminal.write("\x12")?;
            terminal.wait_for("quote history item is visible", |state, _| {
                state.text.contains("Page 1:") && state.text.contains(r#"nchar("``")"#)
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("quote history item replaces buffer", |_, line| {
                line.trim_end() == r#"ARF> nchar("``")"#
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            ensure!(
                terminal.output_since(checkpoint)?.contains("[1] 2"),
                "selected quote-pair history item did not execute"
            );
            Ok(())
        },
    )
}

#[test]
fn history_menu_selection_replaces_auto_matched_paren_pair() -> Result<()> {
    run_case(
        "history-menu-paren-pair",
        &["--no-completion"],
        |terminal| {
            terminal.submit("length(c())", "[1] 0", PROMPT)?;
            let checkpoint = terminal.checkpoint()?;
            terminal.write("c(")?;
            terminal.wait_for("auto-matched paren pair", |_, line| line.contains("c()"))?;
            terminal.write("\x12")?;
            terminal.wait_for("paren history item is visible", |state, _| {
                state.text.contains("Page 1:") && state.text.contains("length(c())")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("paren history item replaces buffer", |_, line| {
                line.trim_end() == "ARF> length(c())"
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            ensure!(
                terminal.output_since(checkpoint)?.contains("[1] 0"),
                "selected paren-pair history item did not execute"
            );
            Ok(())
        },
    )
}

#[test]
fn history_search_preserves_auto_matched_paren_pair() -> Result<()> {
    run_case(
        "history-menu-search-paren-pair",
        &["--no-completion"],
        |terminal| {
            terminal.submit("length(c())", "[1] 0", PROMPT)?;
            let checkpoint = terminal.checkpoint()?;
            terminal.write("\x12")?;
            terminal.wait_for("history menu opens", |state, _| {
                state.text.contains("Page 1:")
            })?;
            terminal.write("c(")?;
            terminal.wait_for("auto-matched paren in history search", |_, line| {
                line.contains("c()")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("search result replaces buffer", |_, line| {
                line.trim_end() == "ARF> length(c())"
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            ensure!(
                terminal.output_since(checkpoint)?.contains("[1] 0"),
                "selected search history item did not execute"
            );
            Ok(())
        },
    )
}
