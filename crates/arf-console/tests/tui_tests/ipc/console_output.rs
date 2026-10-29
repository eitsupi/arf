#![cfg(unix)]

use super::super::support::{ERROR_PROMPT, PROMPT, run_case};
use super::raw_rpc::session_socket_path;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[test]
fn console_output_tracks_terminal_and_ipc_executions_before_completion() -> Result<()> {
    run_case(
        "ipc-console-output",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let (mut reader, connection_id) = attach(terminal)?;

            terminal.submit(
                r#"cat("TERMINAL_OUT|λ"); cat("TERMINAL_ERR|", file = stderr()); cat("TERMINAL_TAIL\n"); cat("TERMINAL_ERR_TAIL\n", file = stderr())"#,
                "TERMINAL_ERR_TAIL",
                PROMPT,
            )?;
            let terminal_messages = read_through_completion(&mut reader)?;
            assert_output_execution(&terminal_messages, json!("terminal"), true, |segments| {
                let all = segment_text(segments);
                let stdout = stream_text(segments, "stdout");
                let stderr = stream_text(segments, "stderr");
                stdout.contains("TERMINAL_OUT|λ")
                    && stdout.contains("TERMINAL_TAIL\n")
                    && stderr.contains("TERMINAL_ERR|TERMINAL_ERR_TAIL\n")
                    && all.find("TERMINAL_OUT|") < all.find("TERMINAL_ERR|")
                    && all.find("TERMINAL_ERR|") < all.find("TERMINAL_TAIL\n")
                    && all.find("TERMINAL_TAIL\n") < all.find("TERMINAL_ERR_TAIL\n")
            })?;

            write_request(
                &mut reader,
                json!({
                    "jsonrpc": "2.0", "id": 2, "method": "evaluate",
                    "params": {"code": r#"cat("SILENT_NO_NEWLINE"); 7"#}
                }),
            )?;
            let silent_messages = collect_ipc_execution(&mut reader, 2, false)?;
            assert_output_execution(&silent_messages, json!(connection_id), false, |segments| {
                segment_text(segments).contains("SILENT_NO_NEWLINE")
            })?;

            // The timer must publish partial output while R is still sleeping.
            write_request(
                &mut reader,
                json!({
                    "jsonrpc": "2.0", "id": 6, "method": "evaluate",
                    "params": {"code": r#"cat("TIMER_FLUSH"); Sys.sleep(5)"#}
                }),
            )?;
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(3)))?;
            let (mut timer_messages, timer_response_seen) =
                read_until_console_output(&mut reader, 6)?;
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(15)))?;
            timer_messages.extend(collect_ipc_execution(&mut reader, 6, timer_response_seen)?);
            assert_output_execution(&timer_messages, json!(connection_id), false, |segments| {
                segment_text(segments).contains("TIMER_FLUSH")
            })?;

            write_request(
                &mut reader,
                json!({
                    "jsonrpc": "2.0", "id": 3, "method": "evaluate",
                    "params": {"code": r#"cat("VISIBLE_OUTPUT\n"); 8"#, "visible": true, "timeout_ms": 10000}
                }),
            )?;
            approve(terminal, "VISIBLE_OUTPUT")?;
            let visible_messages = collect_ipc_execution(&mut reader, 3, false)?;
            assert_output_execution(&visible_messages, json!(connection_id), true, |segments| {
                segment_text(segments).contains("VISIBLE_OUTPUT\n")
            })?;

            write_request(
                &mut reader,
                json!({
                    "jsonrpc": "2.0", "id": 4, "method": "user_input",
                    "params": {"code": r#"cat("USER_INPUT_OUTPUT\n"); 9"#}
                }),
            )?;
            approve(terminal, "USER_INPUT_OUTPUT")?;
            let user_input_messages = collect_ipc_execution(&mut reader, 4, false)?;
            assert_output_execution(
                &user_input_messages,
                json!(connection_id),
                true,
                |segments| segment_text(segments).contains("USER_INPUT_OUTPUT\n"),
            )?;

            write_request(
                &mut reader,
                json!({
                    "jsonrpc": "2.0", "id": 5, "method": "evaluate",
                    "params": {"code": r#"cat("OUTPUT_BEFORE_ERROR"); stop("OUTPUT_TEST_ERROR")"#}
                }),
            )?;
            let failed_messages = collect_ipc_execution(&mut reader, 5, false)?;
            assert_output_execution(&failed_messages, json!(connection_id), false, |segments| {
                segment_text(segments).contains("OUTPUT_BEFORE_ERROR")
            })?;
            let failed_completion = completion(&failed_messages)?;
            ensure!(
                failed_completion["params"]["data"]["status"] == "error",
                "{failed_completion}"
            );

            write_request(
                &mut reader,
                json!({
                    "jsonrpc": "2.0", "id": 7, "method": "user_input",
                    "params": {"code": r#"cat("OUTPUT_BEFORE_INTERRUPT\n"); Sys.sleep(30)"#}
                }),
            )?;
            approve(terminal, "OUTPUT_BEFORE_INTERRUPT")?;
            let (accepted, mut interrupted_messages) = read_response(&mut reader, 7)?;
            let execution_id = accepted["result"]["execution_id"]
                .as_u64()
                .context("user_input acknowledgement lacks execution ID")?;
            ensure!(execution_id > 0, "invalid execution ID: {accepted}");
            let output_already_observed = interrupted_messages.iter().any(|message| {
                message["params"]["topic"] == "arf.console_output"
                    && segment_text(&message["params"]["data"]["segments"])
                        .contains("OUTPUT_BEFORE_INTERRUPT\n")
            });
            if !output_already_observed {
                interrupted_messages.extend(read_until_output_marker(
                    &mut reader,
                    execution_id,
                    "OUTPUT_BEFORE_INTERRUPT\n",
                )?);
            }
            let interrupted = terminal
                .start_ipc(&["interrupt", "--execution-id", &execution_id.to_string()])?
                .finish()?;
            ensure!(
                interrupted["accepted"] == true,
                "interrupt rejected: {interrupted}"
            );
            terminal.wait_for("prompt after output interrupt", |_, line| {
                line.trim_end() == ERROR_PROMPT
            })?;
            interrupted_messages.extend(read_through_completion(&mut reader)?);
            assert_output_execution(
                &interrupted_messages,
                json!(connection_id),
                true,
                |segments| segment_text(segments).contains("OUTPUT_BEFORE_INTERRUPT\n"),
            )?;
            let interrupted_completion = completion(&interrupted_messages)?;
            ensure!(
                interrupted_completion["params"]["data"]["status"] == "interrupted",
                "{interrupted_completion}"
            );

            // An error must not leave buffered output behind for the next execution.
            terminal.submit(r#"cat("AFTER_ERROR_OK")"#, "AFTER_ERROR_OK", PROMPT)?;
            let recovered = read_through_completion(&mut reader)?;
            assert_output_execution(&recovered, json!("terminal"), true, |segments| {
                let text = segment_text(segments);
                text.contains("AFTER_ERROR_OK") && !text.contains("OUTPUT_BEFORE_ERROR")
            })?;
            Ok(())
        },
    )
}

fn attach(terminal: &super::super::support::Terminal) -> Result<(BufReader<UnixStream>, u64)> {
    let stream = UnixStream::connect(session_socket_path(terminal)?)?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut reader = BufReader::new(stream);
    write_request(
        &mut reader,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "attach",
            "params": {"subscribe": ["arf.console_output", "arf.execution_completed"]}
        }),
    )?;
    let response = read_message(&mut reader)?;
    ensure!(response["id"] == 1, "attach failed: {response}");
    let connection_id = response["result"]["connection_id"]
        .as_u64()
        .context("attach response lacks connection ID")?;
    Ok((reader, connection_id))
}

fn write_request(reader: &mut BufReader<UnixStream>, request: Value) -> Result<()> {
    writeln!(reader.get_mut(), "{request}")?;
    reader.get_mut().flush()?;
    Ok(())
}

fn read_message(reader: &mut BufReader<UnixStream>) -> Result<Value> {
    let mut line = String::new();
    ensure!(reader.read_line(&mut line)? > 0, "IPC connection closed");
    Ok(serde_json::from_str(&line)?)
}

fn read_response(reader: &mut BufReader<UnixStream>, id: u64) -> Result<(Value, Vec<Value>)> {
    let mut notifications = Vec::new();
    loop {
        let message = read_message(reader)?;
        if message["id"] == json!(id) {
            return Ok((message, notifications));
        }
        notifications.push(message);
    }
}

fn read_through_completion(reader: &mut BufReader<UnixStream>) -> Result<Vec<Value>> {
    let mut messages = Vec::new();
    loop {
        let message = read_message(reader)?;
        if message["params"]["topic"] == "arf.console_output"
            || message["params"]["topic"] == "arf.execution_completed"
        {
            let is_completion = message["params"]["topic"] == "arf.execution_completed";
            messages.push(message);
            if is_completion {
                return Ok(messages);
            }
        }
    }
}

fn collect_ipc_execution(
    reader: &mut BufReader<UnixStream>,
    id: u64,
    mut response_seen: bool,
) -> Result<Vec<Value>> {
    let mut messages = Vec::new();
    let mut completion_seen = false;
    while !(response_seen && completion_seen) {
        let message = read_message(reader)?;
        if message["id"] == json!(id) {
            response_seen = true;
        }
        if message["params"]["topic"] == "arf.console_output" {
            ensure!(
                !completion_seen,
                "output arrived after completion: {message}"
            );
            messages.push(message);
        } else if message["params"]["topic"] == "arf.execution_completed" {
            completion_seen = true;
            messages.push(message);
        }
    }
    Ok(messages)
}

fn read_until_console_output(
    reader: &mut BufReader<UnixStream>,
    id: u64,
) -> Result<(Vec<Value>, bool)> {
    let mut messages = Vec::new();
    let mut response_seen = false;
    loop {
        let message = read_message(reader)?;
        if message["id"] == json!(id) {
            response_seen = true;
        }
        if message["params"]["topic"] == "arf.console_output" {
            messages.push(message);
            return Ok((messages, response_seen));
        }
        ensure!(
            message["params"]["topic"] != "arf.execution_completed",
            "evaluation completed before timer output was published: {message}"
        );
    }
}

fn read_until_output_marker(
    reader: &mut BufReader<UnixStream>,
    execution_id: u64,
    marker: &str,
) -> Result<Vec<Value>> {
    let mut messages = Vec::new();
    let mut output_text = String::new();
    loop {
        let message = read_message(reader)?;
        if message["params"]["topic"] == "arf.console_output" {
            ensure!(
                message["params"]["execution"] == json!(execution_id),
                "unexpected output execution while waiting for interrupt target: {message}"
            );
            output_text.push_str(&segment_text(&message["params"]["data"]["segments"]));
            messages.push(message);
            if output_text.contains(marker) {
                return Ok(messages);
            }
        } else {
            ensure!(
                message["params"]["topic"] != "arf.execution_completed",
                "interrupt target completed before expected output: {message}"
            );
        }
    }
}

fn completion(messages: &[Value]) -> Result<&Value> {
    messages
        .iter()
        .find(|message| message["params"]["topic"] == "arf.execution_completed")
        .context("completion event missing")
}

fn output_events(messages: &[Value]) -> Vec<&Value> {
    messages
        .iter()
        .filter(|message| message["params"]["topic"] == "arf.console_output")
        .collect()
}

fn assert_output_execution(
    messages: &[Value],
    origin: Value,
    visible: bool,
    check: impl FnOnce(&Value) -> bool,
) -> Result<()> {
    let completed = completion(messages)?;
    let id = completed["params"]["data"]["id"]
        .as_u64()
        .context("completion has no execution ID")?;
    ensure!(completed["params"]["execution"] == json!(id), "{completed}");
    let events = output_events(messages);
    ensure!(
        !events.is_empty(),
        "no console output before completion: {messages:?}"
    );
    for event in &events {
        ensure!(
            event["params"]["execution"] == json!(id),
            "wrong output execution: {event}"
        );
        ensure!(
            event["params"]["data"]["origin"] == origin,
            "wrong output origin: {event}"
        );
        ensure!(
            event["params"]["data"]["visible"] == visible,
            "wrong visible metadata: {event}"
        );
        ensure!(
            event["params"]["data"]["segments"].is_array(),
            "missing output segments: {event}"
        );
        ensure!(
            event["params"]["seq"].as_u64() < completed["params"]["seq"].as_u64(),
            "output must precede completion: {event}; {completed}"
        );
    }
    let combined = Value::Array(
        events
            .into_iter()
            .flat_map(|event| {
                event["params"]["data"]["segments"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
    );
    ensure!(
        check(&combined),
        "output segments did not contain expected text: {combined}"
    );
    Ok(())
}

fn segment_text(segments: &Value) -> String {
    segments
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|segment| segment["text"].as_str())
        .collect()
}

fn stream_text(segments: &Value, stream: &str) -> String {
    segments
        .as_array()
        .into_iter()
        .flatten()
        .filter(|segment| segment["stream"] == stream)
        .filter_map(|segment| segment["text"].as_str())
        .collect()
}

fn approve(terminal: &super::super::support::Terminal, marker: &str) -> Result<()> {
    terminal.wait_for("IPC output approval", |state, line| {
        state.text.contains("IPC send request:")
            && line.contains("Press y to approve")
            && state.text.contains(marker)
    })?;
    terminal.key("y")
}
