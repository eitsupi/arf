use super::support::{DEFAULT_CONFIG, PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Result, ensure};
use std::path::Path;
use tui_test::{MouseAction, Operation, OperationResult};

fn wait_for_prompt_after_ui(terminal: &Terminal, description: &str) -> Result<()> {
    terminal.wait_for(description, |state, line| {
        state.exited.is_none() && line.trim_end() == PROMPT
    })
}

fn wait_for_help_browser_row(terminal: &Terminal, topic: &str) -> Result<()> {
    terminal.wait_for("help browser result row", |state, _| {
        state.text.lines().any(|line| {
            line.trim_start()
                .trim_start_matches("> ")
                .starts_with(&format!("{topic} "))
        })
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
                    && state.text.contains("Filter: _")
                    && state.text.contains("Esc exit")
            })?;
            ensure!(
                terminal.output()?.contains("Help Search"),
                "help search header was not emitted to the terminal stream"
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
fn r_help_keeps_standard_printer_by_default() -> Result<()> {
    run_case(
        "r-help-default",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.submit(
                r#"identical(getS3method("print", "help_files_with_topic"), utils:::print.help_files_with_topic)"#,
                "[1] TRUE",
                PROMPT,
            )
        },
    )
}

#[test]
fn r_help_opens_native_pager_and_q_returns_to_prompt() -> Result<()> {
    run_case_with(
        Terminal::builder("r-help-pager")
            .args(["--no-auto-match", "--no-completion"])
            .config(format!(
                r#"{DEFAULT_CONFIG}
[experimental.r_help]
viewer = "auto"
"#
            )),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter("?mean")?;
            terminal.wait_for_screen_line("mean help pager", 0, |state, line| {
                state.exited.is_none() && line.contains("base::mean")
            })?;
            terminal.key("Enter")?;
            terminal.key("n")?;
            terminal.key("Shift+n")?;
            terminal.write("/qjk/")?;
            terminal.wait_for("pager keys stay in search input", |state, _| {
                state.text.contains("/qjk/|  Enter search  Esc cancel")
            })?;
            terminal.key("Escape")?;
            terminal.wait_for("search input cancels without leaving pager", |state, _| {
                state.text.contains("base::mean") && state.text.contains("q/Esc exit")
            })?;
            terminal.write("/mean")?;
            terminal.key("Enter")?;
            terminal.wait_for("first page search match", |state, _| {
                state.text.contains("/mean [1/")
            })?;
            terminal.key("Enter")?;
            terminal.key("n")?;
            terminal.wait_for("next page search match", |state, _| {
                state.text.contains("/mean [2/")
            })?;
            terminal.key("Shift+n")?;
            terminal.wait_for("previous page search match", |state, _| {
                state.text.contains("/mean [1/")
            })?;
            terminal.write("/arithmetic mean")?;
            terminal.key("Enter")?;
            terminal.wait_for("phrase search before resize", |state, _| {
                state.text.contains("/arithmetic mean [1/")
            })?;
            terminal.execute(Operation::Resize { cols: 45, rows: 18 })?;
            terminal.wait_for(
                "resize keeps phrase search and clears selection",
                |state, _| {
                    state.text.contains("/arithmetic mean [0/")
                        && !state.text.contains("No matches for:")
                },
            )?;
            terminal.key("n")?;
            terminal.wait_for("wrapped phrase remains searchable", |state, _| {
                state.text.contains("/arithmetic mean [1/")
            })?;
            terminal.key("Tab")?;
            terminal.wait_for(
                "selected link action replaces search-only footer",
                |state, _| state.text.contains("Enter open  /arithmetic mean [1/"),
            )?;
            terminal.key("n")?;
            terminal.wait_for(
                "search navigation restores search-only footer",
                |state, _| {
                    state.text.contains("/arithmetic mean [") && !state.text.contains("Enter open")
                },
            )?;
            terminal.key("q")?;
            terminal.wait_for(
                "q clears search and keeps the help page open",
                |state, _| {
                    state.text.contains("base::mean")
                        && state.text.contains("q/Esc exit")
                        && !state.text.contains("n/N next/previous")
                        && !state.text.contains("/arithmetic mean [")
                },
            )?;
            terminal.key("q")?;
            wait_for_prompt_after_ui(terminal, "mean help pager exits")?;
            terminal.enter("?stats::lm")?;
            terminal.wait_for_screen_line("qualified lm help pager", 0, |state, line| {
                state.exited.is_none() && line.contains("stats::lm")
            })?;
            terminal.key("q")?;
            wait_for_prompt_after_ui(terminal, "qualified lm help pager exits")?;
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.quit()
        },
    )
}

fn follow_first_help_link_and_return(terminal: &Terminal) -> Result<()> {
    terminal.key("Enter")?;
    terminal.key("Tab")?;
    terminal.wait_for("R help link is selected", |state, _| {
        state.text.contains("base::mean") && state.text.contains("Enter open")
    })?;
    let state = terminal.state()?;
    let original_header = terminal.screen_line(0, state.cols)?;
    terminal.key("Enter")?;
    terminal.wait_for_screen_line("selected R help link opens", 0, |state, line| {
        state.exited.is_none() && line.contains("::") && !line.contains("base::mean")
    })?;
    let response = terminal
        .start_ipc(&["eval", "1 + 1", "--timeout", "3000"])?
        .finish_with_status()?;
    ensure!(
        response.status.code() == Some(4) && response.json["error"]["code"] == "R_NOT_AT_PROMPT",
        "IPC must stay guarded during navigation: {}",
        response.json
    );
    terminal.key("Backspace")?;
    terminal.wait_for_screen_line("back restores mean title and scroll", 0, |_, line| {
        line == original_header
    })?;
    terminal.wait_for("back restores selected link", |state, _| {
        state.text.contains("Enter open")
    })?;
    Ok(())
}

#[test]
fn r_help_and_browser_share_keyboard_navigation_and_keep_ipc_guarded() -> Result<()> {
    run_case_with(
        Terminal::builder("r-help-navigation")
            .args([
                "--no-auto-match",
                "--no-completion",
                "--with-ipc",
                "--ipc-eval-unrestricted",
            ])
            .config(format!(
                r#"{DEFAULT_CONFIG}
[experimental.r_help]
viewer = "auto"
"#
            )),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter("?mean")?;
            terminal.wait_for_screen_line("mean native help page", 0, |_, line| {
                line.contains("base::mean")
            })?;
            follow_first_help_link_and_return(terminal)?;
            terminal.key("q")?;
            wait_for_prompt_after_ui(terminal, "native help navigation exits")?;

            terminal.enter(":help base::mean")?;
            // Wait for a selectable result row, not just the immediately
            // rendered filter text while the asynchronous search is pending.
            wait_for_help_browser_row(terminal, "base::mean")?;
            // Fuzzy search includes matching aliases such as mean.Date. Select
            // the actual mean row explicitly instead of assuming it ranks first.
            let state = terminal.state()?;
            let mut mean_row = None;
            let mut selected_row = None;
            for row in 0..state.rows.saturating_sub(2) {
                let line = terminal.screen_line(row, state.cols)?;
                if line.trim_start().starts_with("> ") {
                    selected_row = Some(row);
                }
                if line
                    .trim_start()
                    .trim_start_matches("> ")
                    .starts_with("base::mean ")
                {
                    mean_row = Some(row);
                }
            }
            let row = mean_row
                .ok_or_else(|| anyhow::anyhow!("mean row is missing from browser results"))?;
            let selected_row = selected_row
                .ok_or_else(|| anyhow::anyhow!("selected row is missing from browser results"))?;
            for _ in selected_row..row {
                terminal.key("Down")?;
            }
            terminal.wait_for_screen_line("browser explicitly selects mean", row, |_, line| {
                line.trim_start().starts_with("> base::mean ")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_screen_line("browser opens mean page", 0, |_, line| {
                line.contains("base::mean")
            })?;
            follow_first_help_link_and_return(terminal)?;
            terminal.key("q")?;
            terminal.wait_for("navigation returns to topic browser", |state, _| {
                state.text.contains("Filter:") && state.text.contains("Tab/Enter select")
            })?;
            terminal.key("Escape")?;
            wait_for_prompt_after_ui(terminal, "topic browser exits after navigation")?;

            terminal.enter(":help")?;
            terminal.wait_for("empty help browser filter", |state, _| {
                state.text.contains("Filter: _") && state.text.contains("Tab/Enter select")
            })?;
            terminal.write("base::identical")?;
            for _ in 0..3 {
                terminal.key("Backspace")?;
            }
            terminal.wait_for("broadened help browser filter", |state, _| {
                state.text.contains("Filter: base::identi")
            })?;
            terminal.key("Ctrl+u")?;
            terminal.wait_for("cleared help browser filter", |state, _| {
                state.text.contains("Filter: _")
            })?;
            // Enter immediately after the new query to exercise selection while
            // its asynchronous search result is still being produced.
            terminal.write("base::identity")?;
            terminal.key("Enter")?;
            terminal.wait_for_screen_line("identity help page opens", 0, |state, line| {
                state.exited.is_none() && line.contains("base::identity")
            })?;
            terminal.key("q")?;
            terminal.wait_for("identity page returns to browser", |state, _| {
                state.text.contains("Filter: base::identity")
                    && state.text.contains("Tab/Enter select")
            })?;
            terminal.key("Escape")?;
            wait_for_prompt_after_ui(terminal, "help browser regression exits")?;

            let response = terminal
                .start_ipc(&["eval", "1 + 1", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["value"] == "[1] 2",
                "IPC must resume after help exits: {response}"
            );
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.quit()
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
            state.text.contains("Filter: host:")
                && state.text.contains(unique)
                && state.text.contains("q exit")
        })?;
        ensure!(
            terminal.output()?.contains("History Browser"),
            "history browser header was not emitted to the terminal stream"
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
