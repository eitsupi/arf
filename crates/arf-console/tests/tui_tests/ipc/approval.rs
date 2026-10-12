use super::super::support::{ERROR_PROMPT, IpcOutcome, PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Context, Result, ensure};
use serde_json::Value;

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

fn assert_ipc_error(outcome: IpcOutcome, code: &str, message: &str) -> Result<()> {
    ensure!(
        outcome.status.code() == Some(4),
        "IPC error had unexpected exit status: {}",
        outcome.status
    );
    ensure!(
        outcome.json["error"]["code"] == code,
        "wrong IPC error code: {}",
        outcome.json
    );
    ensure!(
        outcome.json["error"]["message"] == message,
        "wrong IPC error message: {}",
        outcome.json
    );
    Ok(())
}

fn history_entry(terminal: &Terminal, marker: &str) -> Result<Value> {
    let response = terminal
        .start_ipc(&[
            "history",
            "--all-sessions",
            "--grep",
            marker,
            "--limit",
            "10",
        ])?
        .finish()?;
    response["entries"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["command"].as_str() == Some(marker))
        })
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("IPC history lacks {marker}: {response}"))
}

fn assert_history_metadata(terminal: &Terminal, marker: &str, expected_status: i64) -> Result<()> {
    let entry = history_entry(terminal, marker)?;
    ensure!(
        entry["timestamp"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "history entry lacks timestamp: {entry}"
    );
    ensure!(
        entry["cwd"].as_str().is_some_and(|value| !value.is_empty()),
        "history entry lacks cwd: {entry}"
    );
    ensure!(
        entry["session_id"].as_i64().is_some(),
        "history entry lacks session_id: {entry}"
    );
    ensure!(
        entry["exit_status"] == expected_status,
        "history entry has wrong exit status: {entry}"
    );
    Ok(())
}

#[test]
fn incomplete_ipc_code_is_rejected_without_entering_continuation() -> Result<()> {
    run_case(
        "ipc-incomplete",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let send = terminal
                .start_ipc(&["send", "foo("])?
                .finish_with_status()?;
            assert_ipc_error(
                send,
                "INCOMPLETE_INPUT",
                "R code is syntactically incomplete",
            )?;

            let evaluate = terminal
                .start_ipc(&["eval", "function(x) {", "--timeout", "10000"])?
                .finish_with_status()?;
            assert_ipc_error(
                evaluate,
                "INCOMPLETE_INPUT",
                "R code is syntactically incomplete",
            )?;

            terminal.wait_for("normal prompt after incomplete IPC", |state, line| {
                line.trim_end() == PROMPT && !state.text.lines().any(|line| line.trim_end() == "+")
            })?;
            terminal.submit(
                "cat('INCOMPLETE_IPC_RECOVERED\\n')",
                "INCOMPLETE_IPC_RECOVERED",
                PROMPT,
            )?;
            Ok(())
        },
    )
}

#[test]
fn approved_ipc_send_is_persisted_with_success_metadata() -> Result<()> {
    let history = tempfile::tempdir()?;
    let marker = "ipc_history_success_marker <- 42; cat('IPC_HISTORY_SUCCESS\\n')";
    run_case_with(
        Terminal::builder("ipc-history-success")
            .args(["--with-ipc", "--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let request = terminal.start_ipc(&["send", marker])?;
            terminal.wait_for("IPC history success approval", |state, _| {
                state.text.contains("IPC send request:")
                    && state.text.contains("Press y to approve")
                    && state.text.contains(marker)
            })?;
            terminal.key("y")?;
            let response = request.finish()?;
            ensure!(
                response["accepted"] == true,
                "history send was not accepted: {response}"
            );
            terminal.wait_for_prompt(Some("IPC_HISTORY_SUCCESS"), PROMPT)?;
            assert_history_metadata(terminal, marker, 0)
        },
    )
}

#[test]
fn approved_ipc_error_is_recorded_and_repl_recovers() -> Result<()> {
    let history = tempfile::tempdir()?;
    let marker = "stop('IPC_HISTORY_ERROR')";
    run_case_with(
        Terminal::builder("ipc-history-error")
            .args(["--with-ipc", "--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let request = terminal.start_ipc(&["send", marker])?;
            terminal.wait_for("IPC history error approval", |state, _| {
                state.text.contains("IPC send request:")
                    && state.text.contains("Press y to approve")
                    && state.text.contains(marker)
            })?;
            terminal.key("y")?;
            let response = request.finish()?;
            ensure!(
                response["accepted"] == true,
                "error send was not accepted: {response}"
            );
            terminal.wait_for_prompt(Some("Error: IPC_HISTORY_ERROR"), ERROR_PROMPT)?;
            assert_history_metadata(terminal, marker, 1)?;
            terminal.submit("42", "[1] 42", PROMPT)
        },
    )
}

#[test]
fn ipc_send_rejects_when_the_user_is_typing_without_moving_the_prompt() -> Result<()> {
    run_case("ipc-user-typing", &["--with-ipc"], |terminal| {
        terminal.wait_for_first_prompt()?;
        let typed = "USER_IS_TYPING_BUFFER";
        terminal.write(typed)?;
        let before = terminal.state()?;
        terminal.wait_for("typed input is rendered", |_, line| line.contains(typed))?;

        let request = terminal.start_ipc(&["send", "1 + 1"])?;
        let outcome = request.finish_with_status()?;
        assert_ipc_error(outcome, "USER_IS_TYPING", "User is typing in the console")?;
        terminal.wait_for("typed input survives IPC rejection", |state, line| {
            line.contains(typed) && state.cursor.y == before.cursor.y
        })?;
        let after = terminal.state()?;
        ensure!(
            after.cursor.y == before.cursor.y,
            "rejected IPC input advanced the prompt row: before={before:?}, after={after:?}"
        );
        ensure!(
            after.text.lines().filter(|line| !line.is_empty()).count()
                <= before.text.lines().filter(|line| !line.is_empty()).count() + 1,
            "rejected IPC input added unexpected screen rows: before={before:?}, after={after:?}"
        );

        terminal.key("Ctrl+C")?;
        terminal.wait_for_prompt(None, PROMPT)?;
        terminal.submit("42", "[1] 42", PROMPT)
    })
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
            state.text.contains("IPC send request:")
                && state.text.contains("Press y to approve")
                && state.text.contains(marker)
        })?;
        terminal.key("n")?;
        assert_ipc_not_approved(request.finish_with_status()?)?;
        terminal.submit(&format!("exists('{marker}')"), "[1] FALSE", PROMPT)
    })
}

#[test]
fn ipc_input_decline_keys_do_not_execute_or_save_code() -> Result<()> {
    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("ipc-send-decline")
            .args(["--with-ipc"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            decline_send_with_key(terminal, "DECLINED_SEND_CTRL_C_MARKER", "Ctrl+C")?;
            decline_send_with_key(terminal, "DECLINED_SEND_ENTER_MARKER", "Enter")?;
            decline_send_with_key(terminal, "DECLINED_SEND_CTRL_D_MARKER", "Ctrl+D")?;

            let history_response = terminal
                .start_ipc(&[
                    "history",
                    "--all-sessions",
                    "--grep",
                    "DECLINED_SEND_",
                    "--limit",
                    "20",
                ])?
                .finish()?;
            let entries = history_response["entries"]
                .as_array()
                .context("IPC history response lacks entries")?;
            ensure!(
                entries.iter().all(|entry| {
                    !entry["command"]
                        .as_str()
                        .is_some_and(|command| command.starts_with("DECLINED_SEND_"))
                }),
                "declined IPC command was persisted in history: {history_response}"
            );

            Ok(())
        },
    )
}

fn decline_send_with_key(terminal: &Terminal, marker: &str, key: &str) -> Result<()> {
    let declined_command = format!(r#"{marker} <- TRUE"#);
    let request = terminal.start_ipc(&["send", &declined_command])?;
    terminal.wait_for("send IPC decline prompt", |state, _| {
        state.text.contains("IPC send request:")
            && state.text.contains("Press y to approve")
            && state.text.contains(marker)
    })?;
    terminal.key(key)?;
    assert_ipc_not_approved(request.finish_with_status()?)?;
    let exists_check = format!(r#"exists("{marker}")"#);
    terminal.submit(&exists_check, "[1] FALSE", PROMPT)
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

            let assignment = "send_policy_allow_echo <- 1";
            let assignment_before = terminal.state()?;
            let assignment_checkpoint = terminal.checkpoint()?;
            let request = terminal.start_ipc(&["send", assignment])?;
            let response = request.finish()?;
            ensure!(
                response["accepted"] == true,
                "policy-allow silent assignment was rejected: {response}"
            );
            terminal.wait_for(
                "policy-allow assignment reaches a fresh prompt",
                |state, line| line.trim_end() == PROMPT && state.text.contains(assignment),
            )?;
            let assignment_state = terminal.state()?;
            ensure!(
                assignment_state.cursor.y > assignment_before.cursor.y,
                "policy-allow assignment did not advance to a fresh screen row: {assignment_state:?}"
            );
            let assignment_lines = assignment_state.text.lines().collect::<Vec<_>>();
            let assignment_line = assignment_lines
                .iter()
                .position(|line| line.starts_with("agent> ") && line.contains(assignment));
            let prompt_line = assignment_lines
                .iter()
                .rposition(|line| line.trim_end() == PROMPT);
            ensure!(
                assignment_line
                    .zip(prompt_line)
                    .is_some_and(|(sent, prompt)| sent < prompt),
                "policy-allow assignment was not echoed before the fresh R prompt: {assignment_state:?}"
            );
            ensure!(
                !terminal
                    .output_since(assignment_checkpoint)?
                    .contains("IPC send request:"),
                "policy-allow silent assignment unexpectedly displayed an approval prompt"
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
        let state = terminal.state()?;
        let output_line = terminal.screen_line(state.cursor.y.saturating_sub(1), state.cols)?;
        ensure!(
            output_line.contains("APPROVED_OUTPUT"),
            "R output should immediately precede the next prompt: {state:?}"
        );
        terminal.submit("ipc_input", "[1] 42", PROMPT)?;

        let marker = "IPC_SILENT_ASSIGNMENT <- 7";
        let assignment_before = terminal.state()?;
        let request = terminal.start_ipc(&["send", marker])?;
        terminal.wait_for("silent IPC assignment approval", |state, _| {
            state.text.contains("IPC send request:")
                && state.text.contains("Press y to approve")
                && state.text.contains(marker)
        })?;
        terminal.key("y")?;
        let response = request.finish()?;
        ensure!(
            response["accepted"] == true,
            "silent assignment was not accepted: {response}"
        );
        terminal.wait_for(
            "silent assignment reaches the next prompt",
            |state, line| line.trim_end() == PROMPT && state.text.contains(marker),
        )?;
        let assignment_state = terminal.state()?;
        ensure!(
            assignment_state.cursor.y > assignment_before.cursor.y,
            "approved assignment did not advance to a fresh screen row: {assignment_state:?}"
        );
        let assignment_lines = assignment_state.text.lines().collect::<Vec<_>>();
        let assignment_line = assignment_lines
            .iter()
            .position(|line| line.starts_with("agent> ") && line.contains(marker));
        let prompt_line = assignment_lines
            .iter()
            .rposition(|line| line.trim_end() == PROMPT);
        ensure!(
            assignment_line
                .zip(prompt_line)
                .is_some_and(|(sent, prompt)| sent < prompt),
            "approved assignment was not echoed before the fresh R prompt: {assignment_state:?}"
        );
        terminal.submit("IPC_SILENT_ASSIGNMENT", "[1] 7", PROMPT)
    })
}
