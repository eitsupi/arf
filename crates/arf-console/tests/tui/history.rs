use super::support::{PROMPT, run_case};
use anyhow::Result;

#[test]
fn history_browser_reopens_after_writing_more_history() -> Result<()> {
    run_case(
        "history-browser",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.submit("1 + 1", "[1] 2", PROMPT)?;
            terminal.submit("print('hello')", "[1] \"hello\"", PROMPT)?;
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
