use super::support::{PROMPT, run_case};
use anyhow::Result;

#[test]
fn arrow_and_backspace_edit_input_before_evaluation() -> Result<()> {
    run_case("editing", &[], |terminal| {
        terminal.write("1 + 40")?;
        terminal.wait_for("input echo", |_, line| line.trim_end() == "ARF> 1 + 40")?;
        terminal.key("Left")?;
        terminal.key("Backspace")?;
        terminal.write("2")?;
        terminal.wait_for("edited input", |_, line| line.trim_end() == "ARF> 1 + 20")?;
        terminal.key("Enter")?;
        terminal.wait_for_prompt(Some("[1] 21"), PROMPT)
    })
}

#[test]
fn ctrl_c_discards_input_and_allows_a_new_command() -> Result<()> {
    run_case("cancel-input", &[], |terminal| {
        terminal.write("unfinished_input")?;
        terminal.wait_for("input before cancellation", |_, line| {
            line.trim_end() == "ARF> unfinished_input"
        })?;
        terminal.key("Ctrl+C")?;
        terminal.wait_for_prompt(None, PROMPT)?;
        terminal.submit(
            "Sys.sleep(0.4); cat(paste0('CANCEL_', 'OK'), '\\n')",
            "CANCEL_OK",
            PROMPT,
        )
    })
}

#[test]
fn multiline_function_uses_continuation_then_returns_to_prompt() -> Result<()> {
    run_case("multiline", &["--no-auto-match"], |terminal| {
        terminal.enter("f <- function(x) {")?;
        terminal.wait_for("continuation prompt", |_, line| line.trim_end() == "+")?;
        terminal.enter("x + 1")?;
        // Require the new body to be echoed, not just the preceding continuation.
        terminal.wait_for("function body and continuation", |state, line| {
            line.trim_end() == "+" && state.text.contains("x + 1")
        })?;
        terminal.enter("}")?;
        terminal.wait_for_prompt(None, PROMPT)?;
        terminal.submit("f(10)", "[1] 11", PROMPT)
    })
}

#[test]
fn unicode_output_preserves_wide_and_combining_characters() -> Result<()> {
    run_case("unicode", &[], |terminal| {
        terminal.submit(
            "cat(intToUtf8(c(26085, 26412, 35486)), '\\n')",
            "日本語",
            PROMPT,
        )?;
        terminal.submit("cat(intToUtf8(c(101, 769)), '\\n')", "e\u{301}", PROMPT)
    })
}
