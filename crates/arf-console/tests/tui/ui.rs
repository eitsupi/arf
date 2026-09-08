use super::support::{PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Result, ensure};
use std::path::Path;
use tui_test::{MouseAction, Operation, OperationResult};

fn wait_for_prompt_after_ui(terminal: &Terminal, description: &str) -> Result<()> {
    terminal.wait_for(description, |state, line| {
        state.exited.is_none() && line.trim_end() == PROMPT
    })
}

fn open_history_schema(terminal: &Terminal) -> Result<()> {
    terminal.enter(":history schema")?;
    terminal.wait_for_screen_line("history schema pager", 0, |state, line| {
        state.exited.is_none() && line.contains("History Schema") && line.contains("[1/")
    })
}

#[test]
fn help_browser_exits_with_escape_and_returns_to_r() -> Result<()> {
    run_case(
        "help-browser",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.enter(":h")?;
            terminal.wait_for("help browser", |state, _| {
                state.exited.is_none()
                    && state.text.contains("Filter: ")
                    && state.text.contains("Esc exit")
            })?;
            ensure!(
                terminal.output()?.contains("Help Search"),
                "help browser title was not rendered"
            );
            terminal.key("Escape")?;
            wait_for_prompt_after_ui(terminal, "help browser exits")?;
            terminal.submit("42", "[1] 42", PROMPT)
        },
    )
}

#[test]
fn history_schema_pager_exits_with_q_and_returns_to_r() -> Result<()> {
    run_case(
        "history-schema",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            open_history_schema(terminal)?;
            terminal.key("q")?;
            wait_for_prompt_after_ui(terminal, "history schema exits")?;
            terminal.submit("42", "[1] 42", PROMPT)
        },
    )
}

#[test]
fn history_schema_copy_uses_clipboard_and_shows_feedback() -> Result<()> {
    run_case(
        "history-schema-copy",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            open_history_schema(terminal)?;
            terminal.key("c")?;
            terminal.wait_for("history schema copy feedback", |state, _| {
                state.text.contains("Copied R example to clipboard")
            })?;
            terminal.execute(Operation::wait_clipboard_match("library(DBI)", Some(5_000)))?;
            let clipboard = match terminal.execute(Operation::GetClipboard)? {
                OperationResult::Clipboard(value) => value,
                result => return Err(anyhow::anyhow!("unexpected clipboard result: {result:?}")),
            };
            ensure!(clipboard.contains("dbConnect("));
            terminal.key("q")?;
            wait_for_prompt_after_ui(terminal, "history schema copy exits")?;
            terminal.submit("42", "[1] 42", PROMPT)
        },
    )
}

#[test]
fn history_schema_mouse_scroll_moves_down_and_up() -> Result<()> {
    run_case(
        "history-schema-mouse",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            open_history_schema(terminal)?;
            let scroll_down = Operation::Mouse {
                action: MouseAction::Scroll {
                    direction: "down".to_owned(),
                    amount: 1,
                },
            };
            ensure!(matches!(
                terminal.execute(scroll_down)?,
                OperationResult::Unit
            ));
            terminal.wait_for_screen_line("history schema scrolls down", 0, |state, line| {
                state.exited.is_none() && line.contains("History Schema") && line.contains("[2/")
            })?;

            let scroll_up = Operation::Mouse {
                action: MouseAction::Scroll {
                    direction: "up".to_owned(),
                    amount: 1,
                },
            };
            ensure!(matches!(
                terminal.execute(scroll_up)?,
                OperationResult::Unit
            ));
            terminal.wait_for_screen_line("history schema scrolls up", 0, |state, line| {
                state.exited.is_none() && line.contains("History Schema") && line.contains("[1/")
            })?;
            terminal.key("q")?;
            wait_for_prompt_after_ui(terminal, "history schema mouse exits")?;
            terminal.submit("42", "[1] 42", PROMPT)
        },
    )
}

#[test]
fn history_browser_persists_between_sessions() -> Result<()> {
    let history = tempfile::tempdir()?;
    let unique = "history_persist_unique_value";
    run_session_with_history("history-persist-write", history.path(), |terminal| {
        terminal.submit(&format!("{unique} <- 123; {unique}"), "[1] 123", PROMPT)?;
        terminal.quit()
    })?;

    run_session_with_history("history-persist-read", history.path(), |terminal| {
        terminal.enter(":history browse")?;
        terminal.wait_for("history browser restored command", |state, _| {
            state.text.contains(unique) && state.text.contains("q exit")
        })?;
        ensure!(
            terminal.output()?.contains("History Browser"),
            "history browser title was not rendered"
        );
        terminal.key("q")?;
        wait_for_prompt_after_ui(terminal, "history browser exits")?;
        terminal.submit("42", "[1] 42", PROMPT)
    })
}

fn run_session_with_history(
    name: &str,
    history: &Path,
    test: impl FnOnce(&Terminal) -> Result<()>,
) -> Result<()> {
    run_case_with(
        Terminal::builder(name)
            .args(["--no-auto-match", "--no-completion"])
            .history_dir(history),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            test(terminal)
        },
    )
}
