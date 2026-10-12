use super::super::support::{PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Context, Result, ensure};
use serde_json::Value;

#[test]
fn silent_ipc_evaluation_captures_output_without_printing_in_the_repl() -> Result<()> {
    run_case(
        "ipc-silent",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            let checkpoint = terminal.checkpoint()?;
            let response = terminal
                .start_ipc(&["eval", "1 + 1", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["error"].is_null(),
                "IPC evaluation error: {response}"
            );
            ensure!(
                response["value"] == "[1] 2",
                "simple IPC value was not captured: {response}"
            );
            ensure!(
                response["stdout"] == "",
                "unexpected simple stdout: {response}"
            );

            let response = terminal
                .start_ipc(&["eval", r"cat('SILENT_IPC_STDOUT\n')", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["stdout"] == "SILENT_IPC_STDOUT\n",
                "wrong captured stdout: {response}"
            );
            ensure!(
                response["error"].is_null(),
                "unexpected cat error: {response}"
            );
            ensure!(
                response["value"].is_null(),
                "unexpected cat value: {response}"
            );

            let response = terminal
                .start_ipc(&["eval", r"stop('SILENT_IPC_ERROR')", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("SILENT_IPC_ERROR")),
                "wrong captured error: {response}"
            );
            ensure!(
                response["value"].is_null(),
                "unexpected error value: {response}"
            );

            let response = terminal
                .start_ipc(&[
                    "eval",
                    r"cat('SILENT_IPC_MIXED\n'); 42",
                    "--timeout",
                    "10000",
                ])?
                .finish()?;
            ensure!(
                response["stdout"] == "SILENT_IPC_MIXED\n",
                "wrong mixed stdout: {response}"
            );
            ensure!(
                response["value"]
                    .as_str()
                    .is_some_and(|value| value == "[1] 42"),
                "wrong mixed value: {response}"
            );
            ensure!(
                response["error"].is_null(),
                "unexpected mixed error: {response}"
            );

            // Cross a visible PTY output boundary before asserting absence:
            // the IPC reply can arrive before the terminal reader catches up.
            let sentinel = terminal.start_ipc(&["send", r"cat('SILENT_IPC_SENTINEL\n')"])?;
            terminal.wait_for("IPC sentinel approval", |state, _| {
                state.text.contains("IPC send request:")
                    && state.text.contains("Press y to approve")
                    && state.text.contains("SILENT_IPC_SENTINEL")
            })?;
            terminal.key("y")?;
            let response = sentinel.finish()?;
            ensure!(
                response["accepted"] == true,
                "sentinel was not accepted: {response}"
            );
            terminal.wait_for_prompt(Some("SILENT_IPC_SENTINEL"), PROMPT)?;
            let visible_output = terminal.output_since(checkpoint)?;
            for marker in ["SILENT_IPC_STDOUT", "SILENT_IPC_ERROR", "SILENT_IPC_MIXED"] {
                ensure!(
                    !visible_output.contains(marker),
                    "silent output leaked to terminal: {marker}"
                );
            }
            Ok(())
        },
    )
}

#[test]
fn visible_ipc_evaluation_waits_for_approval_and_repl_completion() -> Result<()> {
    run_case("ipc-visible", &["--with-ipc"], |terminal| {
        let request = terminal.start_ipc(&[
            "eval",
            r"cat('VISIBLE_IPC_OUTPUT\n'); 99",
            "--visible",
            "--timeout",
            "10000",
        ])?;
        terminal.wait_for("visible IPC approval", |state, _| {
            state.text.contains("IPC send request:")
                && state.text.contains("Press y to approve")
                && state.text.contains("VISIBLE_IPC_OUTPUT")
        })?;
        terminal.key("y")?;

        // The client reply is deliberately collected before polling the PTY:
        // visible evaluation replies only after R completes, while the reader
        // may still be catching up with the rendered output.
        let response = request.finish()?;
        ensure!(
            response["stdout"]
                .as_str()
                .is_some_and(|stdout| stdout.contains("VISIBLE_IPC_OUTPUT")),
            "visible reply missed cat output: {response}"
        );
        ensure!(
            response["stdout"]
                .as_str()
                .is_some_and(|stdout| stdout.contains("[1] 99")),
            "visible reply missed auto-print value: {response}"
        );
        ensure!(
            response["value"].is_null(),
            "visible reply has a value: {response}"
        );
        ensure!(
            response["error"].is_null(),
            "visible reply has an error: {response}"
        );

        terminal.wait_for("visible output reaches the R prompt", |state, line| {
            state.text.contains("VISIBLE_IPC_OUTPUT")
                && state.text.contains("[1] 99")
                && line.trim_end() == PROMPT
        })?;
        let state = terminal.state()?;
        let output_line = terminal.screen_line(state.cursor.y.saturating_sub(1), state.cols)?;
        ensure!(
            output_line.trim_end() == "[1] 99",
            "R output should immediately precede the prompt without a blank line: {state:?}",
        );

        // The same code is rejected on the unattended path in a restricted
        // session; visible evaluation above is intentionally approval-based.
        let restricted = terminal
            .start_ipc(&[
                "eval",
                r"cat('VISIBLE_IPC_OUTPUT\n'); 99",
                "--timeout",
                "10000",
            ])?
            .finish_with_status()?;
        ensure!(
            restricted.status.code() == Some(4),
            "restricted evaluation had unexpected exit status: {}",
            restricted.status
        );
        ensure!(
            restricted.json["error"]["code"] == "R_EVAL_NOT_ALLOWED",
            "wrong restricted-evaluation error code: {}",
            restricted.json
        );
        ensure!(
            restricted.json["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("rejected by policy")),
            "wrong restricted-evaluation error message: {}",
            restricted.json
        );
        Ok(())
    })
}

fn json_object_from_cli_text(text: &str) -> Result<Value> {
    let start = text
        .find('{')
        .context("CLI PTY output lacks a structured JSON object")?;
    let end = text
        .rfind('}')
        .context("CLI PTY output lacks the end of its JSON object")?;
    ensure!(end >= start, "CLI PTY JSON bounds are invalid: {text:?}");
    Ok(serde_json::from_str(&text[start..=end])?)
}

#[test]
fn ipc_eval_without_code_reports_structured_tty_error() -> Result<()> {
    run_case("ipc-eval-no-code-tty", &["--with-ipc"], |terminal| {
        let cli = terminal.start_cli_tty(&["ipc", "eval"])?;
        ensure!(
            cli.wait_for_exit()? == 2,
            "IPC eval without code should exit with client status 2"
        );
        let response = json_object_from_cli_text(&cli.state()?.text)?;
        ensure!(
            response["error"]["code"] == "NO_CODE_PROVIDED",
            "wrong no-code error: {response}"
        );
        ensure!(
            response["error"]["message"] == "No code provided and stdin is a terminal",
            "wrong no-code message: {response}"
        );
        terminal.submit("42", "[1] 42", PROMPT)
    })
}

fn row_text(cells: &[tui_test::Cell]) -> String {
    cells.iter().map(|cell| cell.char.as_str()).collect()
}

fn has_indexed_foreground(cell: &tui_test::Cell, index: u8) -> bool {
    cell.fg == tui_test::CellColor::Indexed(index)
}

#[test]
fn ipc_approval_prompt_preserves_full_code_and_expected_styles() -> Result<()> {
    run_case_with(
        Terminal::builder("ipc-approval-styles")
            .args(["--with-ipc"])
            .env("NO_COLOR", ""),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let code = format!(
                "style_approval_prefix <- 1; {}style_approval_tail <- 1",
                "style_approval_padding <- 1; ".repeat(10)
            );
            let checkpoint = terminal.checkpoint()?;
            let request = terminal.start_ipc(&["send", &code])?;
            terminal.wait_for("long IPC approval prompt", |state, _| {
                state.text.contains("IPC send request:")
                    && state.text.contains("Press y to approve")
                    && state.text.contains("style_approval_prefix")
                    && state.text.contains("style_approval_tail")
            })?;
            let state = terminal.state()?;
            let mut heading_styled = false;
            let mut code_styled = false;
            let mut confirmation_styled = false;
            for y in 0..state.rows {
                let cells = terminal.screen_cells(y, state.cols)?;
                let text = row_text(&cells);
                if text.contains("IPC send request:") {
                    heading_styled = cells.iter().any(|cell| has_indexed_foreground(cell, 6));
                }
                if text.contains("style_approval_prefix") {
                    code_styled = cells.iter().any(|cell| has_indexed_foreground(cell, 11));
                }
                if text.contains("Press y to approve") {
                    confirmation_styled = cells
                        .iter()
                        .any(|cell| has_indexed_foreground(cell, 11) && cell.bold);
                }
            }
            ensure!(
                heading_styled,
                "approval heading was not dark cyan: {state:?}"
            );
            ensure!(code_styled, "approval code was not yellow: {state:?}");
            ensure!(
                confirmation_styled,
                "approval confirmation was not bold yellow: {state:?}"
            );
            ensure!(
                terminal.output_since(checkpoint)?.contains(&code),
                "long approval code was truncated in terminal output"
            );
            terminal.key("y")?;
            ensure!(
                request.finish()?["accepted"] == true,
                "long approval request was not accepted"
            );
            terminal.wait_for("long approval returns to prompt", |state, line| {
                state.text.contains("style_approval_tail") && line.trim_end() == PROMPT
            })?;
            Ok(())
        },
    )
}

#[test]
fn dropping_pending_ipc_client_can_be_cancelled_and_repl_recovers() -> Result<()> {
    run_case("ipc-drop-pending", &["--with-ipc"], |terminal| {
        terminal.wait_for_first_prompt()?;
        let request = terminal.start_ipc(&["send", "1 + 1"])?;
        terminal.wait_for("dropped IPC client approval", |state, _| {
            state.text.contains("IPC send request:") && state.text.contains("Press y to approve")
        })?;
        drop(request);
        terminal.key("Ctrl+C")?;
        terminal.wait_for("prompt after dropped IPC client", |_, line| {
            line.trim_end() == PROMPT
        })?;
        terminal.submit("42", "[1] 42", PROMPT)
    })
}
