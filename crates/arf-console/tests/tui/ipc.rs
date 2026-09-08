use super::support::{
    ERROR_PROMPT, IpcOutcome, PROMPT, Terminal, run_case, run_case_with, wait_for_path_absent,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

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

#[cfg(unix)]
fn custom_bind_path(root: &Path) -> String {
    root.join("arf-tui-custom.sock").display().to_string()
}

#[cfg(windows)]
fn custom_bind_path(root: &Path) -> String {
    let unique = root
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "session".into());
    format!(r"\\.\pipe\arf-tui-{unique}")
}

fn session_metadata_path(terminal: &Terminal) -> Result<PathBuf> {
    Ok(terminal.sessions_dir().join(format!(
        "{}.json",
        terminal.pid().context("missing arf PID")?
    )))
}

fn assert_pid_file(terminal: &Terminal, pid_path: &Path) -> Result<PathBuf> {
    ensure!(
        pid_path.is_file(),
        "PID file was not ready by the first prompt: {}",
        pid_path.display()
    );
    let pid = terminal.pid().context("missing arf PID")?;
    ensure!(
        fs::read_to_string(pid_path)?.trim() == pid.to_string(),
        "PID file does not identify this process"
    );
    let metadata = session_metadata_path(terminal)?;
    ensure!(
        metadata.is_file(),
        "session metadata was not written: {}",
        metadata.display()
    );
    Ok(metadata)
}

#[test]
fn custom_ipc_bind_uses_cli_transport_and_cleans_up_metadata() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let bind_path = custom_bind_path(temp.path());
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-custom-bind").args([
            "--with-ipc",
            "--ipc-eval-unrestricted",
            "--ipc-bind",
            bind_path.as_str(),
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let metadata = session_metadata_path(terminal)?;
            let metadata_json: Value = serde_json::from_str(&fs::read_to_string(&metadata)?)?;
            ensure!(
                metadata_json["socket_path"] == bind_path,
                "session metadata has wrong bind path: {metadata_json}"
            );
            let session = terminal.start_ipc(&["session"])?.finish()?;
            ensure!(
                session["socket_path"] == bind_path,
                "session response has wrong bind path: {session}"
            );
            let response = terminal
                .start_ipc(&["eval", "1 + 1", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["value"] == "[1] 2" && response["error"].is_null(),
                "custom bind IPC evaluation failed: {response}"
            );
            terminal.submit(
                "cat('CUSTOM_BIND_REPL_READY\\n')",
                "CUSTOM_BIND_REPL_READY",
                PROMPT,
            )?;
            metadata_path = Some(metadata);
            terminal.key("Ctrl+D")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "custom-bind Ctrl+D did not exit successfully"
            );
            Ok(())
        },
    )?;
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("custom-bind metadata path was not captured")?,
    )?;
    // The interactive exit path removes session metadata, but currently
    // leaves a custom Unix socket pathname behind. Do not delete or hide that
    // product lifecycle gap in this migration test.
    Ok(())
}

#[test]
fn ipc_pid_file_is_created_and_removed_after_q_exit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let pid_path = temp.path().join("arf.pid");
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-pid-file-q").args([
            "--with-ipc",
            "--ipc-pid-file",
            pid_path.to_string_lossy().as_ref(),
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            metadata_path = Some(assert_pid_file(terminal, &pid_path)?);
            terminal.enter("q('no')")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "q() did not exit successfully"
            );
            Ok(())
        },
    )?;
    wait_for_path_absent(&pid_path)?;
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("q() metadata path was not captured")?,
    )?;
    Ok(())
}

#[test]
fn ipc_pid_file_is_removed_after_ctrl_d_exit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let pid_path = temp.path().join("arf.pid");
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-pid-file-ctrl-d").args([
            "--with-ipc",
            "--ipc-pid-file",
            pid_path.to_string_lossy().as_ref(),
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            metadata_path = Some(assert_pid_file(terminal, &pid_path)?);
            terminal.key("Ctrl+D")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "Ctrl+D did not exit successfully"
            );
            Ok(())
        },
    )?;
    wait_for_path_absent(&pid_path)?;
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("Ctrl+D metadata path was not captured")?,
    )?;
    Ok(())
}

#[test]
fn existing_ipc_pid_file_is_rejected_without_mutation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let pid_path = temp.path().join("arf.pid");
    let sentinel = "existing-pid-file-sentinel";
    fs::write(&pid_path, sentinel)?;
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-pid-file-existing").args([
            "--with-ipc",
            "--ipc-pid-file",
            pid_path.to_string_lossy().as_ref(),
        ]),
        |terminal| {
            let pid = terminal.pid().context("missing arf PID")?;
            metadata_path = Some(session_metadata_path(terminal)?);
            let exit_code = terminal.wait_for_exit()?;
            ensure!(
                exit_code != 0,
                "startup unexpectedly succeeded with an existing PID file (pid {pid})"
            );
            ensure!(
                fs::read_to_string(&pid_path)? == sentinel,
                "existing PID file was modified"
            );
            Ok(())
        },
    )?;
    ensure!(
        fs::read_to_string(&pid_path)? == sentinel,
        "existing PID file was modified after process exit"
    );
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("rejected-startup metadata path was not captured")?,
    )?;
    Ok(())
}

#[test]
fn incomplete_ipc_code_is_rejected_without_entering_continuation() -> Result<()> {
    run_case(
        "ipc-incomplete",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let checkpoint = terminal.checkpoint()?;

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
            ensure!(
                terminal
                    .output_since(checkpoint)?
                    .contains("INCOMPLETE_IPC_RECOVERED"),
                "follow-up output was not observed after incomplete IPC requests"
            );
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
fn ipc_input_ctrl_c_decline_does_not_execute_code() -> Result<()> {
    run_case("ipc-send-decline", &["--with-ipc"], |terminal| {
        let marker = "DECLINED_SEND_IPC_MARKER";
        let request = terminal.start_ipc(&["send", &format!("{marker} <- TRUE")])?;
        terminal.wait_for("send IPC decline prompt", |state, _| {
            state.text.contains("IPC send request:")
                && state.text.contains("Press y to approve")
                && state.text.contains(marker)
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
        let state = terminal.state()?;
        let output_line = terminal.screen_line(state.cursor.y.saturating_sub(1), state.cols)?;
        ensure!(
            output_line.contains("APPROVED_OUTPUT"),
            "R output should immediately precede the next prompt: {state:?}"
        );
        terminal.submit("ipc_input", "[1] 42", PROMPT)?;

        let marker = "IPC_SILENT_ASSIGNMENT <- 7";
        let checkpoint = terminal.checkpoint()?;
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
        ensure!(
            terminal.output_since(checkpoint)?.contains(marker),
            "silent IPC assignment echo was not retained"
        );
        terminal.submit("IPC_SILENT_ASSIGNMENT", "[1] 7", PROMPT)
    })
}
