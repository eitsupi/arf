use super::super::support::{PROMPT, Terminal, run_case};
use super::raw_rpc::{raw_attach, raw_json_rpc, session_socket_path};
use anyhow::{Result, ensure};
use serde_json::json;

fn wait_for_expose_confirmation(terminal: &Terminal) -> Result<()> {
    terminal.wait_for("handler publication confirmation", |_, line| {
        line.contains("Publish these IPC handlers? [y/N]")
    })
}

fn wait_for_message_prompt(terminal: &Terminal, marker: &str, checkpoint: usize) -> Result<()> {
    terminal.wait_for("message and input prompt", |_, line| {
        line.trim_end() == PROMPT
            && terminal
                .output_since(checkpoint)
                .is_ok_and(|output| output.contains(marker))
    })
}

#[test]
fn interactive_exposure_is_separate_from_send_policy_and_uses_existing_handler_lifecycle()
-> Result<()> {
    run_case(
        "ipc-interactive-expose",
        &["--with-ipc", "--no-auto-match"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let socket = session_socket_path(terminal)?;
            terminal.submit(
                ":ipc send-policy allow",
                "# [arf] IPC send policy: allow",
                PROMPT,
            )?;

            terminal.enter(r#":ipc expose { side_effect <- 41; list("demo/retry" = function(args) side_effect, "demo/line\nbreak" = function(args) "kept  spaces") }"#)?;
            wait_for_expose_confirmation(terminal)?;
            let preview = terminal.state()?.text;
            ensure!(
                preview
                    .lines()
                    .any(|line| line == r"# [arf]   demo/line\nbreak"),
                "control character in candidate name was not escaped: {preview}"
            );
            let checkpoint = terminal.checkpoint()?;
            terminal.key("Ctrl+C")?;
            wait_for_message_prompt(terminal, "exposure declined", checkpoint)?;

            terminal.submit(
                r#"cat("EXPOSE_SIDE_EFFECT:", side_effect, "\n", sep = "")"#,
                "EXPOSE_SIDE_EFFECT:41",
                PROMPT,
            )?;

            terminal.enter(r#":ipc expose list("demo/retry" = function(args) side_effect, "demo/line\nbreak" = function(args) "kept  spaces")"#)?;
            wait_for_expose_confirmation(terminal)?;
            let blocked = raw_json_rpc(&socket, "evaluate", json!({"code": "side_effect <- 99"}))?;
            ensure!(
                blocked["error"].is_object(),
                "IPC evaluation ran during handler confirmation: {blocked}"
            );
            let checkpoint = terminal.checkpoint()?;
            terminal.key("y")?;
            wait_for_message_prompt(terminal, "IPC handlers exposed.", checkpoint)?;

            let call = raw_json_rpc(&socket, "call", json!({"handler":"demo/retry"}))?;
            ensure!(
                call["result"]["value"] == json!(41.0),
                "pending IPC evaluation changed the candidate environment: {call}"
            );
            terminal.submit_quiet("gc()", PROMPT)?;
            let escaped_call =
                raw_json_rpc(&socket, "call", json!({"handler":"demo/line\nbreak"}))?;
            ensure!(
                escaped_call["result"]["value"] == json!("kept  spaces"),
                "candidate handler was not retained through gc(): {escaped_call}"
            );
            let session = raw_json_rpc(&socket, "session", json!({}))?;
            ensure!(
                session["result"]["handler_api"]["state"] == "locked",
                "{session}"
            );
            ensure!(
                session["result"]["handler_api"]["handlers"]
                    == json!(["demo/retry", "demo/line\nbreak"]),
                "{session}"
            );
            let attached = raw_attach(&socket)?;
            ensure!(
                attached["result"]["capabilities"]
                    .as_array()
                    .is_some_and(|capabilities| capabilities
                        .iter()
                        .any(|value| value == "handlers")),
                "attached connection did not advertise handlers: {attached}"
            );

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(
                r#":ipc expose { before_locked <- 8; list("demo/late" = function(args) 8) }"#,
            )?;
            wait_for_message_prompt(terminal, "Failed to prepare IPC handlers:", checkpoint)?;
            terminal.submit(
                r#"cat("LOCKED_PRECHECK:", exists("before_locked", inherits = FALSE), "\n", sep = "")"#,
                "LOCKED_PRECHECK:FALSE",
                PROMPT,
            )?;

            terminal.submit(":ipc status", "# [arf] IPC handlers: Locked", PROMPT)?;
            terminal.submit(
                ":ipc revoke",
                "# [arf] IPC handlers revoked for this process.",
                PROMPT,
            )?;
            let session = raw_json_rpc(&socket, "session", json!({}))?;
            ensure!(
                session["result"]["handler_api"]["state"] == "revoked",
                "{session}"
            );

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(
                r#":ipc expose { after_revoke <- 9; list("demo/late" = function(args) 9) }"#,
            )?;
            wait_for_message_prompt(terminal, "Failed to prepare IPC handlers:", checkpoint)?;
            terminal.submit(
                r#"cat("REVOKED_PRECHECK:", exists("after_revoke", inherits = FALSE), "\n", sep = "")"#,
                "REVOKED_PRECHECK:FALSE",
                PROMPT,
            )?;
            Ok(())
        },
    )
}

#[test]
fn interrupted_handler_expression_leaves_registry_unexposed_and_retryable() -> Result<()> {
    run_case(
        "ipc-interactive-expose-interrupt",
        &["--with-ipc", "--no-auto-match"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let socket = session_socket_path(terminal)?;
            terminal.enter(
                r#":ipc expose { cat("EXPOSE_EVAL_STARTED\n"); repeat { Sys.sleep(0.1) } }"#,
            )?;
            terminal.wait_for("handler expression started", |state, _| {
                state.text.lines().any(|line| line == "EXPOSE_EVAL_STARTED")
            })?;
            let checkpoint = terminal.checkpoint()?;
            terminal.key("Ctrl+C")?;
            wait_for_message_prompt(terminal, "Failed to prepare IPC handlers:", checkpoint)?;

            let session = raw_json_rpc(&socket, "session", json!({}))?;
            ensure!(
                session["result"]["handler_api"]["state"] == "unexposed",
                "{session}"
            );
            terminal
                .enter(r#":ipc expose list("demo/retry-after-interrupt" = function(args) 42)"#)?;
            wait_for_expose_confirmation(terminal)?;
            let checkpoint = terminal.checkpoint()?;
            terminal.key("y")?;
            wait_for_message_prompt(terminal, "IPC handlers exposed.", checkpoint)?;
            let call = raw_json_rpc(
                &socket,
                "call",
                json!({"handler":"demo/retry-after-interrupt"}),
            )?;
            ensure!(call["result"]["value"] == json!(42.0), "{call}");
            Ok(())
        },
    )
}

#[test]
fn interactive_exposure_errors_and_ipc_code_cannot_publish_handlers() -> Result<()> {
    run_case(
        "ipc-interactive-expose-errors",
        &["--with-ipc", "--ipc-eval-unrestricted", "--no-auto-match"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":ipc expose 42")?;
            wait_for_message_prompt(terminal, "Failed to prepare IPC handlers:", checkpoint)?;

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(r#":ipc expose stop("EXPOSE_EVALUATION_ERROR")"#)?;
            wait_for_message_prompt(terminal, "Failed to prepare IPC handlers:", checkpoint)?;

            // IPC evaluation is R input and cannot dispatch a terminal meta command.
            let socket = session_socket_path(terminal)?;
            let response = raw_json_rpc(
                &socket,
                "evaluate",
                json!({"code": r#":ipc expose list("demo/injected" = function(args) 1)"#}),
            )?;
            ensure!(
                response["error"].is_object() || response["result"]["error"].is_string(),
                "injected meta command unexpectedly succeeded: {response}"
            );
            let session = raw_json_rpc(&socket, "session", json!({}))?;
            ensure!(
                session["result"]["handler_api"]["state"] == "unexposed",
                "{session}"
            );

            terminal.submit("42", "[1] 42", PROMPT)?;
            Ok(())
        },
    )
}

#[cfg(unix)]
#[test]
fn exposure_does_not_leak_execution_context_into_terminal_or_handler_events() -> Result<()> {
    use super::completion::read_notification_timeout;
    use anyhow::Context;
    use std::io::{BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    run_case(
        "ipc-expose-execution-context",
        &["--with-ipc", "--no-auto-match"],
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let socket = session_socket_path(terminal)?;
            let mut stream = UnixStream::connect(&socket)?;
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
            let attached = read_notification_timeout(&mut reader)?.context("attach response")?;
            ensure!(attached["id"] == 1, "attach failed: {attached}");

            terminal.enter(
                r#":ipc expose { .Call("arf_publish_event", "demo.expose", NULL, PACKAGE = "(embedding)"); list("demo/context" = function(args) { .Call("arf_publish_event", "demo.handler", NULL, PACKAGE = "(embedding)"); list(value = 42L) }) }"#,
            )?;
            wait_for_expose_confirmation(terminal)?;
            let expose_event = read_notification_timeout(&mut reader)?.context("exposure event")?;
            ensure!(
                expose_event["params"]["topic"] == "demo.expose",
                "unexpected exposure notification: {expose_event}"
            );
            ensure!(
                expose_event["params"]["execution"].is_null(),
                "exposure evaluation inherited an execution context: {expose_event}"
            );
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            ensure!(
                read_notification_timeout(&mut reader)?.is_none(),
                "exposure emitted a completion"
            );
            let checkpoint = terminal.checkpoint()?;
            terminal.key("y")?;
            wait_for_message_prompt(terminal, "IPC handlers exposed.", checkpoint)?;

            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            terminal.submit(
                r#"cat("EXPOSE_CONTEXT_TERMINAL\n"); .Call("arf_publish_event", "demo.terminal", NULL, PACKAGE = "(embedding)" )"#,
                "EXPOSE_CONTEXT_TERMINAL",
                PROMPT,
            )?;
            let terminal_event =
                read_notification_timeout(&mut reader)?.context("terminal event")?;
            let completion =
                read_notification_timeout(&mut reader)?.context("terminal completion")?;
            ensure!(
                terminal_event["params"]["topic"] == "demo.terminal",
                "unexpected terminal event: {terminal_event}"
            );
            ensure!(
                completion["params"]["topic"] == "arf.execution_completed",
                "terminal execution did not complete: {completion}"
            );
            let terminal_id = completion["params"]["data"]["id"]
                .as_u64()
                .context("terminal completion lacks execution ID")?;
            ensure!(
                terminal_id > 0,
                "invalid terminal execution ID: {terminal_id}"
            );
            ensure!(completion["params"]["execution"] == json!(terminal_id));
            ensure!(terminal_event["params"]["execution"] == json!(terminal_id));
            ensure!(completion["params"]["data"]["origin"] == "terminal");

            let call = raw_json_rpc(&socket, "call", json!({"handler":"demo/context"}))?;
            ensure!(call["result"]["value"] == json!({"value": 42}), "{call}");
            let handler_id = call["result"]["execution_id"]
                .as_u64()
                .context("handler response lacks execution ID")?;
            ensure!(handler_id != terminal_id, "{call}");
            let handler_event = read_notification_timeout(&mut reader)?.context("handler event")?;
            ensure!(
                handler_event["params"]["topic"] == "demo.handler",
                "unexpected handler event: {handler_event}"
            );
            ensure!(handler_event["params"]["execution"] == json!(handler_id));
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            ensure!(
                read_notification_timeout(&mut reader)?.is_none(),
                "handler call emitted a completion"
            );
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            terminal.submit(
                r#"cat("AFTER_HANDLER_CONTEXT\n"); .Call("arf_publish_event", "demo.after_handler", NULL, PACKAGE = "(embedding)")"#,
                "AFTER_HANDLER_CONTEXT",
                PROMPT,
            )?;
            let after_event =
                read_notification_timeout(&mut reader)?.context("event after handler")?;
            let after_completion =
                read_notification_timeout(&mut reader)?.context("completion after handler")?;
            let after_id = after_completion["params"]["data"]["id"]
                .as_u64()
                .context("execution ID after handler")?;
            ensure!(
                after_id != handler_id && after_id != terminal_id,
                "context was reused: {after_completion}"
            );
            ensure!(
                after_completion["params"]["topic"] == "arf.execution_completed",
                "{after_completion}"
            );
            ensure!(
                after_event["params"]["topic"] == "demo.after_handler",
                "{after_event}"
            );
            ensure!(after_event["params"]["execution"] == json!(after_id));
            ensure!(after_completion["params"]["execution"] == json!(after_id));
            ensure!(after_completion["params"]["data"]["origin"] == "terminal");
            Ok(())
        },
    )
}
