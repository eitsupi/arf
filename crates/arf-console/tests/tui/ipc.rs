use super::support::{IpcOutcome, PROMPT, Terminal, run_case};
use anyhow::{Result, ensure};

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
            state.text.contains("IPC send request:") && state.text.contains("VISIBLE_IPC_OUTPUT")
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

fn assert_ipc_not_approved(outcome: IpcOutcome) -> Result<()> {
    ensure!(
        outcome.status.code() == Some(4),
        "declined IPC request had unexpected exit status: {}",
        outcome.status
    );
    ensure!(
        outcome.json["error"]["code"] == "INPUT_NOT_APPROVED",
        "wrong declined IPC error code: {}",
        outcome.json
    );
    ensure!(
        outcome.json["error"]["message"] == "IPC send was not approved",
        "wrong declined IPC error message: {}",
        outcome.json
    );
    Ok(())
}

#[test]
fn visible_ipc_evaluation_decline_does_not_execute_code() -> Result<()> {
    run_case("ipc-visible-decline", &["--with-ipc"], |terminal| {
        let marker = "DECLINED_VISIBLE_IPC_MARKER";
        let request = terminal.start_ipc(&[
            "eval",
            &format!("{marker} <- TRUE"),
            "--visible",
            "--timeout",
            "10000",
        ])?;
        terminal.wait_for("visible IPC decline prompt", |state, _| {
            state.text.contains("IPC send request:") && state.text.contains(marker)
        })?;
        terminal.key("n")?;
        assert_ipc_not_approved(request.finish_with_status()?)?;
        terminal.submit(&format!("exists('{marker}')"), "[1] FALSE", PROMPT)
    })
}

#[test]
fn ipc_input_ctrl_c_decline_does_not_execute_code() -> Result<()> {
    run_case("ipc-send-decline", &["--with-ipc"], |terminal| {
        let marker = "DECLINED_SEND_IPC_MARKER";
        let request = terminal.start_ipc(&["send", &format!("{marker} <- TRUE")])?;
        terminal.wait_for("send IPC decline prompt", |state, _| {
            state.text.contains("IPC send request:") && state.text.contains(marker)
        })?;
        terminal.key("Ctrl+C")?;
        assert_ipc_not_approved(request.finish_with_status()?)?;
        terminal.submit(&format!("exists('{marker}')"), "[1] FALSE", PROMPT)
    })
}

fn visible_policy(terminal: &Terminal) -> Result<String> {
    let response = terminal.start_ipc(&["session"])?.finish()?;
    response["ipc_policy"]["visible"]["mode"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("session response lacks visible policy: {response}"))
}

fn wait_for_info_policy(terminal: &Terminal, expected: &str) -> Result<()> {
    terminal.enter(":info")?;
    terminal.key("G")?;
    terminal.wait_for("session info visible policy", |state, _| {
        state.text.contains(expected)
    })?;
    terminal.key("q")?;
    terminal.wait_for_prompt(None, PROMPT)
}

fn wait_for_policy_command(terminal: &Terminal, policy: &str) -> Result<()> {
    terminal.wait_for("send policy confirmation", |state, line| {
        state.text.contains(policy) && line.trim_end() == PROMPT
    })
}

#[test]
fn live_send_policy_controls_ipc_approval_and_session_info() -> Result<()> {
    run_case(
        "ipc-send-policy",
        &["--with-ipc", "--no-auto-match", "--no-completion"],
        |terminal| {
            ensure!(
                visible_policy(terminal)? == "approval_required",
                "initial visible policy should require approval"
            );

            terminal.enter(":ipc send-policy allow")?;
            wait_for_policy_command(terminal, "IPC send policy: allow")?;
            ensure!(
                visible_policy(terminal)? == "approval_not_required",
                "allow policy was not reflected by the live session"
            );

            let send_checkpoint = terminal.checkpoint()?;
            let request = terminal.start_ipc(&["send", r"cat('POLICY_SEND_OUTPUT\n')"])?;
            let response = request.finish()?;
            ensure!(
                response["accepted"] == true,
                "policy-allow send was rejected: {response}"
            );
            terminal.wait_for_prompt(Some("POLICY_SEND_OUTPUT"), PROMPT)?;
            ensure!(
                !terminal
                    .output_since(send_checkpoint)?
                    .contains("IPC send request:"),
                "allow policy unexpectedly displayed an approval prompt"
            );

            let eval_checkpoint = terminal.checkpoint()?;
            let response = terminal
                .start_ipc(&[
                    "eval",
                    r"cat('POLICY_VISIBLE_OUTPUT\n'); 7",
                    "--visible",
                    "--timeout",
                    "10000",
                ])?
                .finish()?;
            ensure!(
                response["stdout"]
                    .as_str()
                    .is_some_and(|stdout| stdout.contains("POLICY_VISIBLE_OUTPUT")),
                "policy-allow visible eval missed stdout: {response}"
            );
            ensure!(
                response["value"].is_null(),
                "visible eval returned a value: {response}"
            );
            ensure!(
                response["error"].is_null(),
                "visible eval returned an error: {response}"
            );
            terminal.wait_for_prompt(Some("POLICY_VISIBLE_OUTPUT"), PROMPT)?;
            ensure!(
                !terminal
                    .output_since(eval_checkpoint)?
                    .contains("IPC send request:"),
                "allow policy unexpectedly displayed an eval approval prompt"
            );

            wait_for_info_policy(terminal, "Visible requests: approval not required")?;

            terminal.enter(":ipc send-policy prompt")?;
            wait_for_policy_command(terminal, "IPC send policy: prompt")?;
            ensure!(
                visible_policy(terminal)? == "approval_required",
                "prompt policy was not reflected by the live session"
            );
            wait_for_info_policy(terminal, "Visible requests: approval required")
        },
    )
}

#[test]
fn approved_ipc_input_is_evaluated_before_the_next_prompt() -> Result<()> {
    run_case("ipc-approval", &["--with-ipc"], |terminal| {
        let request = terminal.start_ipc(&[
            "send",
            r"ipc_input <- 42; Sys.sleep(0.4); cat(paste0('APPROVED_', 'OUTPUT'), '\n')",
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
