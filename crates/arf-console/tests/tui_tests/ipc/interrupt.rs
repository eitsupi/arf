use super::super::support::{ERROR_PROMPT, PROMPT, run_case};
use anyhow::{Context, Result, ensure};
use std::time::{Duration, Instant};

fn wait_for_file(path: &std::path::Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        ensure!(
            Instant::now() < deadline,
            "evaluation marker was not created: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

#[test]
fn ipc_interrupt_stops_visible_evaluation_and_preserves_the_repl() -> Result<()> {
    run_case(
        "ipc-interrupt-visible",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            let evaluation = terminal.start_ipc(&[
                "eval",
                r#"cat("IPC_INTERRUPT_STARTED\n"); Sys.sleep(30)"#,
                "--visible",
                "--timeout",
                "30000",
            ])?;
            terminal.wait_for("visible IPC approval", |state, line| {
                state.text.contains("IPC send request:") && line.contains("Press y to approve")
            })?;
            terminal.key("y")?;
            terminal.wait_for("visible IPC evaluation starts", |state, _| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim() == "IPC_INTERRUPT_STARTED")
            })?;

            let interrupted = terminal.start_ipc(&["interrupt"])?.finish()?;
            ensure!(
                interrupted["accepted"] == true,
                "active IPC evaluation was not interrupted: {interrupted}"
            );
            let result = evaluation.finish_with_status()?;
            ensure!(
                result.json["error"]
                    .as_str()
                    .is_some_and(|error| error.to_ascii_lowercase().contains("interrupt")),
                "visible evaluation did not report interruption: {}",
                result.json
            );

            terminal.wait_for("prompt after IPC interrupt", |_, line| {
                matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
            })?;
            terminal.submit("3 * 7", "[1] 21", PROMPT)?;

            let nested = terminal.start_ipc(&[
                "eval",
                r#"readline("IPC_NESTED_READLINE? "); IPC_NESTED_REMAINDER <- TRUE"#,
                "--visible",
                "--timeout",
                "30000",
            ])?;
            terminal.wait_for("nested visible IPC approval", |state, line| {
                state.text.contains("IPC send request:") && line.contains("Press y to approve")
            })?;
            terminal.key("y")?;
            terminal.wait_for("nested readline prompt", |state, _| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim().starts_with("IPC_NESTED_READLINE?"))
            })?;
            let interrupted = terminal.start_ipc(&["interrupt"])?.finish()?;
            ensure!(
                interrupted["accepted"] == true,
                "nested IPC evaluation was not interrupted: {interrupted}"
            );
            let result = nested.finish_with_status()?;
            ensure!(
                result.json["error"]
                    .as_str()
                    .is_some_and(|error| error.to_ascii_lowercase().contains("interrupt")),
                "nested evaluation did not report interruption: {}",
                result.json
            );
            terminal.wait_for("prompt after nested IPC interrupt", |_, line| {
                line.trim_end() == ERROR_PROMPT
            })?;
            terminal.submit(
                "exists('IPC_NESTED_REMAINDER', envir = .GlobalEnv)",
                "[1] FALSE",
                PROMPT,
            )
        },
    )
}

#[test]
fn ipc_interrupt_targets_silent_send_and_not_terminal_code() -> Result<()> {
    run_case(
        "ipc-interrupt-origins",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            let silent_dir = tempfile::tempdir()?;
            let silent_marker = silent_dir.path().join("started");
            let silent_path = serde_json::to_string(&silent_marker.to_string_lossy().as_ref())?;
            let silent_code = format!("file.create({silent_path}); Sys.sleep(30)");
            let silent = terminal.start_ipc(&["eval", &silent_code, "--timeout", "30000"])?;
            wait_for_file(&silent_marker)?;
            let stopped = terminal.start_ipc(&["interrupt"])?.finish()?;
            ensure!(
                stopped["accepted"] == true,
                "silent interrupt rejected: {stopped}"
            );
            let result = silent.finish_with_status()?;
            ensure!(
                result.json["error"]
                    .as_str()
                    .is_some_and(|error| error.to_ascii_lowercase().contains("interrupt")),
                "silent evaluation did not report interruption: {}",
                result.json
            );

            let send_code = r#"cat("IPC_INTERRUPT_SEND_STARTED\n"); Sys.sleep(30)"#;
            let send = terminal.start_ipc(&["send", send_code])?;
            terminal.wait_for("send approval before interrupt", |state, _| {
                state.text.contains("IPC send request:")
                    && state.text.contains("Press y to approve")
            })?;
            terminal.key("y")?;
            terminal.wait_for("IPC send evaluation starts", |state, _| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim() == "IPC_INTERRUPT_SEND_STARTED")
            })?;
            let accepted_send = send.finish()?;
            let execution_id = accepted_send["execution_id"]
                .as_u64()
                .context("accepted user_input lacks execution ID")?;
            let stopped = terminal
                .start_ipc(&["interrupt", "--execution-id", &execution_id.to_string()])?
                .finish()?;
            ensure!(
                stopped["accepted"] == true,
                "send interrupt rejected: {stopped}"
            );
            terminal.wait_for("prompt after send interrupt", |_, line| {
                line.trim_end() == ERROR_PROMPT
            })?;

            terminal.enter(r#"cat("IPC_TERMINAL_INTERRUPT_TARGET\n"); Sys.sleep(2)"#)?;
            terminal.wait_for("terminal evaluation starts", |state, _| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim() == "IPC_TERMINAL_INTERRUPT_TARGET")
            })?;
            let rejected = terminal.start_ipc(&["interrupt"])?.finish()?;
            ensure!(
                rejected["accepted"] == false,
                "interrupt must not target terminal input: {rejected}"
            );
            terminal.wait_for("terminal evaluation finishes", |_, line| {
                line.trim_end() == PROMPT
            })?;
            terminal.submit("5 * 5", "[1] 25", PROMPT)
        },
    )
}

#[cfg(unix)]
#[test]
fn ipc_interrupt_reports_interrupted_completion_for_visible_and_send() -> Result<()> {
    use super::completion::{read_notification_timeout, send_json_line};
    use super::raw_rpc::session_socket_path;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    /// Wait for the response to `request_id` and the completion event of the
    /// execution that response names, ignoring other executions' completions.
    fn await_response_and_completion(
        reader: &mut BufReader<UnixStream>,
        request_id: u64,
    ) -> Result<(Value, Value)> {
        let mut response: Option<Value> = None;
        let mut completions = Vec::new();
        loop {
            if let Some(response) = &response {
                let execution_id = &response["result"]["execution_id"];
                if let Some(completion) = completions
                    .iter()
                    .find(|completion: &&Value| completion["params"]["data"]["id"] == *execution_id)
                {
                    return Ok((response.clone(), completion.clone()));
                }
            }
            let message = read_notification_timeout(reader)?
                .context("timed out waiting for the IPC response and completion")?;
            if message["method"] == "event" {
                if message["params"]["topic"] == "arf.execution_completed" {
                    completions.push(message);
                }
            } else if message["id"] == request_id {
                response = Some(message);
            }
        }
    }

    run_case(
        "ipc-interrupt-completion",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let mut stream = UnixStream::connect(session_socket_path(terminal)?)?;
            stream.set_read_timeout(Some(Duration::from_secs(20)))?;
            writeln!(
                stream,
                "{}",
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "attach",
                    "params": {"subscribe": ["arf.execution_completed"]}
                })
            )?;
            stream.flush()?;
            let mut reader = BufReader::new(stream);
            let mut attached = String::new();
            reader.read_line(&mut attached)?;
            let attached: Value = serde_json::from_str(&attached)?;
            ensure!(attached["id"] == 1, "attach failed: {attached}");

            // Visible evaluation: interrupted, with the earlier assignment kept.
            send_json_line(
                &mut reader,
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "evaluate",
                    "params": {
                        "code": r#"interrupt_kept <- 42L; cat("IPC_COMPLETION_VISIBLE\n"); Sys.sleep(30)"#,
                        "visible": true,
                        "timeout_ms": 30000
                    }
                }),
            )?;
            terminal.wait_for("visible IPC approval", |state, line| {
                state.text.contains("IPC send request:") && line.contains("Press y to approve")
            })?;
            terminal.key("y")?;
            terminal.wait_for("visible evaluation starts", |state, _| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim() == "IPC_COMPLETION_VISIBLE")
            })?;
            ensure!(terminal.start_ipc(&["interrupt"])?.finish()?["accepted"] == true);
            let (response, completion) = await_response_and_completion(&mut reader, 2)?;
            ensure!(
                response["result"]["error"]
                    .as_str()
                    .is_some_and(|error| error.to_ascii_lowercase().contains("interrupt")),
                "{response}"
            );
            ensure!(
                completion["params"]["data"]["status"] == "interrupted",
                "visible interrupt must report interrupted: {completion}"
            );
            terminal.wait_for("prompt after visible interrupt", |_, line| {
                matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
            })?;
            terminal.submit("interrupt_kept", "[1] 42", PROMPT)?;

            // Send: interrupted, and the remaining line of the input never runs.
            send_json_line(
                &mut reader,
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "user_input",
                    "params": {
                        "code": "cat('IPC_COMPLETION_SEND\\n'); Sys.sleep(30)\ninterrupt_remainder <- TRUE"
                    }
                }),
            )?;
            terminal.wait_for("send approval", |state, line| {
                state.text.contains("IPC send request:") && line.contains("Press y to approve")
            })?;
            terminal.key("y")?;
            terminal.wait_for("send evaluation starts", |state, _| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim() == "IPC_COMPLETION_SEND")
            })?;
            ensure!(terminal.start_ipc(&["interrupt"])?.finish()?["accepted"] == true);
            let (_, completion) = await_response_and_completion(&mut reader, 3)?;
            ensure!(
                completion["params"]["data"]["status"] == "interrupted",
                "send interrupt must report interrupted: {completion}"
            );
            terminal.wait_for("prompt after send interrupt", |_, line| {
                matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
            })?;
            terminal.submit(
                "exists('interrupt_remainder', envir = .GlobalEnv)",
                "[1] FALSE",
                PROMPT,
            )?;

            // An ordinary error in a visible evaluation is still an error.
            send_json_line(
                &mut reader,
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "evaluate",
                    "params": {
                        "code": r#"stop("ordinary visible failure")"#,
                        "visible": true,
                        "timeout_ms": 30000
                    }
                }),
            )?;
            terminal.wait_for("error evaluation approval", |state, line| {
                state.text.contains("IPC send request:") && line.contains("Press y to approve")
            })?;
            terminal.key("y")?;
            let (_, completion) = await_response_and_completion(&mut reader, 4)?;
            ensure!(
                completion["params"]["data"]["status"] == "error",
                "ordinary errors must stay errors: {completion}"
            );
            Ok(())
        },
    )
}
