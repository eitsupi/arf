use super::super::support::{PROMPT, Terminal, run_case_with};
use super::raw_rpc::{raw_json_rpc, session_socket_path};
use anyhow::{Result, ensure};
use serde_json::json;

#[test]
fn exposed_handlers_preserve_typed_input_capture_output_and_can_be_revoked() -> Result<()> {
    run_case_with(
        Terminal::builder("ipc-handlers-typing").args([
            "--with-ipc",
            "--ipc-expose",
            r#"list("demo/echo" = function(x) {cat("HANDLER_HIDDEN\n"); message("HANDLER_MESSAGE"); x})"#,
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let socket = session_socket_path(terminal)?;
            terminal.write("typed_value <- ")?;
            terminal.wait_for("partially typed assignment", |_, line| line.contains("typed_value <-"))?;
            let checkpoint = terminal.checkpoint()?;
            let response = raw_json_rpc(&socket, "call", json!({
                "handler": "demo/echo", "args": {"n": 42}, "queue_timeout_ms": 5000
            }))?;
            ensure!(response["result"]["value"] == json!({"n":42.0}), "{response}");
            ensure!(response["result"]["stdout"] == "HANDLER_HIDDEN\n", "{response}");
            ensure!(response["result"]["stderr"].as_str().is_some_and(|s| s.contains("HANDLER_MESSAGE")), "{response}");
            // Completing the same editor buffer proves it survived the call.
            terminal.write(r#"42; cat("TYPED_OK:", typed_value, "\n", sep = "")"#)?;
            terminal.key("enter")?;
            terminal.wait_for_prompt(Some("TYPED_OK:42"), PROMPT)?;
            let output = terminal.output_since(checkpoint)?;
            ensure!(!output.contains("HANDLER_HIDDEN") && !output.contains("HANDLER_MESSAGE"), "handler output leaked: {output}");

            let history = terminal.start_ipc(&["history", "--limit", "1000"])?.finish()?;
            ensure!(history["entries"].as_array().is_some_and(|rows| rows.len() == 1), "call entered history: {history}");
            terminal.enter(":ipc revoke")?;
            // A completion menu can consume the first Enter for meta commands.
            terminal.key("enter")?;
            terminal.wait_for_prompt(Some("# [arf] IPC handlers revoked for this process."), PROMPT)?;
            let response = raw_json_rpc(&socket, "call", json!({"handler":"demo/echo"}))?;
            ensure!(response["error"]["code"] == -32010, "{response}");
            let session = raw_json_rpc(&socket, "session", json!({}))?;
            ensure!(session["result"]["handler_api"]["state"] == "revoked", "{session}");
            Ok(())
        },
    )
}
