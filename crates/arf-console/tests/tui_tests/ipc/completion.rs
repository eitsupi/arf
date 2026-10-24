#![cfg(unix)]

use super::super::support::{ERROR_PROMPT, PROMPT, run_case};
use super::raw_rpc::session_socket_path;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::time::Duration;

#[cfg(unix)]
#[test]
fn terminal_code_completes_once_and_empty_input_does_not_complete() -> Result<()> {
    use std::os::unix::net::UnixStream;

    run_case(
        "ipc-execution-completion",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let mut stream = UnixStream::connect(session_socket_path(terminal)?)?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            writeln!(
                stream,
                "{}",
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "attach",
                    "params": {"subscribe": ["demo.*", "arf.execution_completed"]}
                })
            )?;
            stream.flush()?;
            let mut reader = BufReader::new(stream);
            let mut attached_line = String::new();
            reader.read_line(&mut attached_line)?;
            let attached: Value = serde_json::from_str(&attached_line)?;
            ensure!(attached["id"] == 1, "attach failed: {attached}");
            let connection_id = attached["result"]["connection_id"]
                .as_u64()
                .context("attach response lacks connection ID")?;

            // The initial prompt and a submitted blank line have no R
            // evaluation to complete.
            terminal.submit_quiet("", PROMPT)?;
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            ensure!(
                read_notification_timeout(&mut reader)?.is_none(),
                "empty input emitted an event"
            );

            // Silent IPC evaluation receives the same execution context and
            // publishes its completion after any events emitted by its code.
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            send_json_line(
                &mut reader,
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "evaluate",
                    "params": {
                        "code": r#".Call("arf_publish_event", "demo.silent", NULL, PACKAGE = "(embedding)"); 3"#
                    }
                }),
            )?;
            let (silent_result, silent_event, silent_completion) =
                collect_completion(&mut reader, 2, "demo.silent")?;
            assert_completion(
                &silent_result,
                &silent_event,
                &silent_completion,
                connection_id,
                false,
            )?;

            // Visible IPC evaluation goes through the REPL and waits for the
            // session's existing approval prompt.
            send_json_line(
                &mut reader,
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "evaluate",
                    "params": {
                        "code": r#".Call("arf_publish_event", "demo.visible", NULL, PACKAGE = "(embedding)"); 4"#,
                        "visible": true,
                        "timeout_ms": 10000
                    }
                }),
            )?;
            terminal.wait_for("visible IPC approval", |state, line| {
                state.text.contains("IPC send request:")
                    && line.contains("Press y to approve")
                    && state.text.contains("demo.visible")
            })?;
            terminal.key("y")?;
            let (visible_result, visible_event, visible_completion) =
                collect_completion(&mut reader, 3, "demo.visible")?;
            assert_completion(
                &visible_result,
                &visible_event,
                &visible_completion,
                connection_id,
                true,
            )?;

            // Empty IPC input gets its normal accepted acknowledgment but no
            // execution event. The next terminal command still has its own
            // context and completion.
            send_json_line(
                &mut reader,
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "user_input",
                    "params": {"code": ""}
                }),
            )?;
            terminal.wait_for("empty user_input approval", |state, line| {
                state.text.contains("IPC send request:") && line.contains("Press y to approve")
            })?;
            terminal.key("y")?;
            let accepted: Value = read_notification_timeout(&mut reader)?
                .context("empty user_input was not acknowledged")?;
            ensure!(accepted["id"] == 4, "{accepted}");
            ensure!(accepted["result"]["accepted"] == true, "{accepted}");
            ensure!(
                accepted["result"]["execution_id"].as_u64().is_some(),
                "{accepted}"
            );
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            ensure!(
                read_notification_timeout(&mut reader)?.is_none(),
                "empty user_input emitted an event"
            );

            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            terminal.submit(
                r#".Call("arf_publish_event", "demo.terminal", NULL, PACKAGE = "(embedding)"); 2"#,
                "[1] 2",
                PROMPT,
            )?;

            let first = read_notification_timeout(&mut reader)?
                .context("terminal R event was not published")?;
            let second = read_notification_timeout(&mut reader)?
                .context("terminal completion event was not published")?;
            let (r_event, completion) = if first["params"]["topic"] == "demo.terminal" {
                (first, second)
            } else {
                (second, first)
            };
            ensure!(r_event["params"]["topic"] == "demo.terminal", "{r_event}");
            ensure!(
                completion["params"]["topic"] == "arf.execution_completed",
                "{completion}"
            );
            let execution_id = completion["params"]["data"]["id"]
                .as_u64()
                .context("completion event lacks execution ID")?;
            ensure!(completion["params"]["execution"] == json!(execution_id));
            ensure!(completion["params"]["data"]["origin"] == "terminal");
            ensure!(completion["params"]["data"]["kind"] == "user_code");
            ensure!(completion["params"]["data"]["visible"] == true);
            ensure!(completion["params"]["data"]["status"] == "ok");
            ensure!(r_event["params"]["execution"] == json!(execution_id));
            let event_seq = r_event["params"]["seq"]
                .as_u64()
                .context("R notification lacks sequence number")?;
            let completion_seq = completion["params"]["seq"]
                .as_u64()
                .context("completion event lacks sequence number")?;
            ensure!(
                event_seq < completion_seq,
                "R event should precede completion: event={r_event}, completion={completion}"
            );
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            ensure!(
                read_notification_timeout(&mut reader)?.is_none(),
                "one terminal input emitted more than one completion"
            );
            ensure!(
                connection_id > 0,
                "unexpected connection ID: {connection_id}"
            );

            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            terminal.submit(
                "stop('TERMINAL_EXECUTION_ERROR')",
                "Error: TERMINAL_EXECUTION_ERROR",
                ERROR_PROMPT,
            )?;
            let failed = read_notification_timeout(&mut reader)?
                .context("terminal error completion event was not published")?;
            ensure!(
                failed["params"]["topic"] == "arf.execution_completed",
                "{failed}"
            );
            ensure!(failed["params"]["data"]["origin"] == "terminal");
            ensure!(failed["params"]["data"]["status"] == "error", "{failed}");
            let failed_id = failed["params"]["data"]["id"]
                .as_u64()
                .context("terminal error completion lacks execution ID")?;
            ensure!(failed["params"]["execution"] == json!(failed_id));
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            ensure!(
                read_notification_timeout(&mut reader)?.is_none(),
                "terminal error emitted duplicate completion events"
            );

            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            terminal.submit("2 + 3", "[1] 5", PROMPT)?;
            let recovered = read_notification_timeout(&mut reader)?
                .context("terminal recovery completion event was not published")?;
            ensure!(
                recovered["params"]["topic"] == "arf.execution_completed",
                "{recovered}"
            );
            let recovered_id = recovered["params"]["data"]["id"]
                .as_u64()
                .context("recovery completion lacks execution ID")?;
            ensure!(recovered_id != failed_id, "{failed}; {recovered}");
            ensure!(recovered["params"]["execution"] == json!(recovered_id));
            ensure!(recovered["params"]["data"]["origin"] == "terminal");
            ensure!(recovered["params"]["data"]["status"] == "ok", "{recovered}");
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            ensure!(
                read_notification_timeout(&mut reader)?.is_none(),
                "terminal recovery emitted duplicate completion events"
            );
            Ok(())
        },
    )
}

#[cfg(unix)]
fn send_json_line(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
    request: Value,
) -> Result<()> {
    writeln!(reader.get_mut(), "{request}")?;
    reader.get_mut().flush()?;
    Ok(())
}

#[cfg(unix)]
fn collect_completion(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
    request_id: u64,
    event_topic: &str,
) -> Result<(Value, Value, Value)> {
    let mut response = None;
    let mut r_event = None;
    let mut completion = None;
    while response.is_none() || r_event.is_none() || completion.is_none() {
        let message =
            read_notification_timeout(reader)?.context("timed out waiting for IPC result")?;
        if message["method"] == "event" {
            match message["params"]["topic"].as_str() {
                Some(topic) if topic == event_topic => r_event = Some(message),
                Some("arf.execution_completed") => completion = Some(message),
                _ => {}
            }
        } else if message["id"] == request_id {
            response = Some(message);
        }
    }
    Ok((response.unwrap(), r_event.unwrap(), completion.unwrap()))
}

#[cfg(unix)]
fn assert_completion(
    response: &Value,
    r_event: &Value,
    completion: &Value,
    origin: u64,
    visible: bool,
) -> Result<()> {
    ensure!(
        response["error"].is_null(),
        "IPC evaluation failed: {response}"
    );
    let execution_id = response["result"]["execution_id"]
        .as_u64()
        .context("evaluate response lacks execution ID")?;
    ensure!(completion["params"]["data"]["id"] == json!(execution_id));
    ensure!(completion["params"]["execution"] == json!(execution_id));
    ensure!(completion["params"]["data"]["origin"] == json!(origin));
    ensure!(completion["params"]["data"]["visible"] == visible);
    ensure!(completion["params"]["data"]["status"] == "ok");
    ensure!(r_event["params"]["execution"] == json!(execution_id));
    let event_seq = r_event["params"]["seq"]
        .as_u64()
        .context("R event lacks sequence number")?;
    let completion_seq = completion["params"]["seq"]
        .as_u64()
        .context("completion event lacks sequence number")?;
    ensure!(
        event_seq < completion_seq,
        "{r_event} followed by {completion}"
    );
    Ok(())
}

#[cfg(unix)]
fn read_notification_timeout(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
) -> Result<Option<Value>> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => anyhow::bail!("IPC connection closed"),
        Ok(_) => Ok(Some(serde_json::from_str(&line)?)),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}
