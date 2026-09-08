use super::support::{PROMPT, run_case};
use anyhow::{Result, ensure};

#[test]
fn silent_ipc_evaluation_captures_output_without_printing_in_the_repl() -> Result<()> {
    run_case(
        "ipc-silent",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            let checkpoint = terminal.checkpoint()?;
            let response = terminal
                .start_ipc(&[
                    "eval",
                    "ipc_value <- 41; cat('SILENT_IPC_OUTPUT\\n'); ipc_value + 1",
                    "--timeout",
                    "10000",
                ])?
                .finish()?;
            ensure!(
                response["error"].is_null(),
                "IPC evaluation error: {response}"
            );
            ensure!(
                response["stdout"] == "SILENT_IPC_OUTPUT\n",
                "wrong captured stdout: {response}"
            );
            ensure!(
                response["value"]
                    .as_str()
                    .is_some_and(|value| value.contains("42")),
                "wrong value: {response}"
            );
            // Cross a visible PTY output boundary before asserting absence:
            // the IPC reply can arrive before the terminal reader catches up.
            terminal.submit("ipc_value", "[1] 41", PROMPT)?;
            ensure!(
                !terminal
                    .output_since(checkpoint)?
                    .contains("SILENT_IPC_OUTPUT"),
                "silent output leaked to terminal"
            );
            Ok(())
        },
    )
}

#[test]
fn approved_ipc_input_is_evaluated_before_the_next_prompt() -> Result<()> {
    run_case("ipc-approval", &["--with-ipc"], |terminal| {
        let request = terminal.start_ipc(&[
            "send",
            "ipc_input <- 42; Sys.sleep(0.4); cat(paste0('APPROVED_', 'OUTPUT'), '\\n')",
        ])?;
        terminal.wait_for("IPC approval prompt", |state, _| {
            state.text.contains("IPC send request:") && state.text.contains("Press y to approve")
        })?;
        terminal.key("y")?;
        let response = request.finish()?;
        ensure!(
            response["accepted"] == true,
            "input was not accepted: {response}"
        );
        // Acceptance is not completion. Require output and the actual input prompt.
        terminal.wait_for_prompt(Some("APPROVED_OUTPUT"), PROMPT)?;
        terminal.submit("ipc_input", "[1] 42", PROMPT)
    })
}
