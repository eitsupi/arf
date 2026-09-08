use super::support::{PROMPT, run_case};
use anyhow::{Result, ensure};

#[test]
fn recording_retains_output_erased_from_the_screen() -> Result<()> {
    run_case("erased-output", &["--no-auto-match"], |terminal| {
        let checkpoint = terminal.checkpoint()?;
        // Observe the text before clearing it. ConPTY may coalesce unobserved
        // intermediate screen updates into a single output update.
        terminal.submit(
            r"cat(paste0('ERASED_', 'OUTPUT'), '\n')",
            "ERASED_OUTPUT",
            PROMPT,
        )?;
        terminal.submit(
            r"cat('\033[2J\033[H', paste0('VISIBLE_', 'OUTPUT'), '\n', sep='')",
            "VISIBLE_OUTPUT",
            PROMPT,
        )?;
        ensure!(
            !terminal.state()?.text.contains("ERASED_OUTPUT"),
            "erased text remains on screen"
        );
        let output = terminal.output_since(checkpoint)?;
        ensure!(
            output.matches("ERASED_OUTPUT").count() == 1,
            "missing/duplicated erased output: {output:?}"
        );
        let checkpoint = terminal.checkpoint()?;
        terminal.submit(
            r"cat(paste0('SECOND_', 'OUTPUT'), '\n')",
            "SECOND_OUTPUT",
            PROMPT,
        )?;
        let output = terminal.output_since(checkpoint)?;
        ensure!(
            !output.contains("ERASED_OUTPUT"),
            "checkpoint included old output"
        );
        ensure!(
            output.contains("SECOND_OUTPUT"),
            "checkpoint missed new output"
        );
        Ok(())
    })
}

#[test]
// arf installs its terminal askpass handler only on Unix. Windows uses a GUI
// dialog, whose interaction is outside a terminal-based test suite.
#[cfg(unix)]
fn askpass_does_not_echo_the_password_in_terminal_output() -> Result<()> {
    run_case("askpass", &["--no-auto-match"], |terminal| {
        // askpass is part of the declared test dependencies; absence is a failure.
        terminal.submit(
            "print(requireNamespace('askpass', quietly = TRUE))",
            "[1] TRUE",
            PROMPT,
        )?;
        let checkpoint = terminal.checkpoint()?;
        terminal.enter("askpass::askpass(paste0('Enter ', 'password: '))")?;
        terminal.wait_for("password prompt", |_, line| {
            line.trim_end() == "Enter password:"
        })?;
        terminal.enter("secret_answer")?;
        terminal.wait_for_prompt(Some(r#"[1] "secret_answer""#), PROMPT)?;
        let output = terminal.output_since(checkpoint)?;
        let quoted = output.matches(r#""secret_answer""#).count();
        ensure!(
            quoted > 0 && output.matches("secret_answer").count() == quoted,
            "password was echoed: {output:?}"
        );
        Ok(())
    })
}
