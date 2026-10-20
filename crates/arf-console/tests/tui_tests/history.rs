use super::support::{PROMPT, run_case};
use anyhow::Result;

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
fn history_menu_selection_replaces_existing_buffer() -> Result<()> {
    let command = "history_menu_replace_value <- 999";
    run_case(
        "history-menu-replace",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.submit_quiet(command, PROMPT)?;
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
            // Include a counter in the history item so the replay has a
            // result that cannot be confused with the initial evaluation.
            let command = r#"history_q <- get0("history_q", ifnotfound = 0) + 1; nchar("``") + history_q - 1"#;
            terminal.submit(command, "[1] 2", PROMPT)?;
            terminal.write("`")?;
            terminal.wait_for("auto-matched quote pair", |_, line| line.contains("``"))?;
            terminal.write("\x12")?;
            terminal.wait_for("quote history item is visible", |state, _| {
                state.text.contains("Page 1:") && state.text.contains(command)
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("quote history item replaces buffer", |_, line| {
                line.trim_end() == format!("ARF> {command}")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(Some("[1] 3"), PROMPT)?;
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
            let command = r#"history_p <- get0("history_p", ifnotfound = 0) + 1; length(c()) + history_p - 1"#;
            terminal.submit(command, "[1] 0", PROMPT)?;
            terminal.write("c(")?;
            terminal.wait_for("auto-matched paren pair", |_, line| line.contains("c()"))?;
            terminal.write("\x12")?;
            terminal.wait_for("paren history item is visible", |state, _| {
                state.text.contains("Page 1:") && state.text.contains(command)
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("paren history item replaces buffer", |_, line| {
                line.trim_end() == format!("ARF> {command}")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(Some("[1] 1"), PROMPT)?;
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
            let command = r#"history_s <- get0("history_s", ifnotfound = 0) + 1; length(c()) + history_s - 1"#;
            terminal.submit(command, "[1] 0", PROMPT)?;
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
                line.trim_end() == format!("ARF> {command}")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(Some("[1] 1"), PROMPT)?;
            Ok(())
        },
    )
}

#[test]
fn history_recall_keeps_quote_pair_backspace_state_in_sync() -> Result<()> {
    run_case(
        "history-recall-quote-pair-backspace",
        &["--no-completion"],
        |terminal| {
            terminal.write(r#"""#)?;
            terminal.wait_for("empty auto-matched quote pair", |_, line| {
                line.trim_end() == r#"ARF> """#
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(Some(r#"[1] """#), PROMPT)?;
            terminal.submit("1", "[1] 1", PROMPT)?;

            terminal.key("Up")?;
            terminal.key("Up")?;
            terminal.wait_for("recalled empty quote pair", |_, line| {
                line.trim_end() == r#"ARF> """#
            })?;
            terminal.key("Down")?;
            terminal.key("Up")?;
            terminal.wait_for("quote pair survives history navigation", |_, line| {
                line.trim_end() == r#"ARF> """#
            })?;
            terminal.key("Left")?;
            terminal.key("Backspace")?;
            terminal.wait_for("recalled quote pair is deleted", |_, line| {
                line.trim_end() == PROMPT
            })?;
            terminal.submit("1+1", "[1] 2", PROMPT)
        },
    )
}

#[test]
fn history_recall_keeps_bracket_pair_backspace_state_in_sync() -> Result<()> {
    run_case(
        "history-recall-bracket-pair-backspace",
        &["--no-completion"],
        |terminal| {
            terminal.write("{")?;
            terminal.wait_for("empty auto-matched bracket pair", |_, line| {
                line.trim_end() == "ARF> {}"
            })?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(Some("NULL"), PROMPT)?;
            terminal.submit("1", "[1] 1", PROMPT)?;

            terminal.key("Up")?;
            terminal.key("Up")?;
            terminal.wait_for("recalled empty bracket pair", |_, line| {
                line.trim_end() == "ARF> {}"
            })?;
            terminal.key("Down")?;
            terminal.key("Up")?;
            terminal.wait_for("bracket pair survives history navigation", |_, line| {
                line.trim_end() == "ARF> {}"
            })?;
            terminal.key("Left")?;
            terminal.key("Backspace")?;
            terminal.wait_for("recalled bracket pair is deleted", |_, line| {
                line.trim_end() == PROMPT
            })?;
            terminal.submit("1+1", "[1] 2", PROMPT)
        },
    )
}
