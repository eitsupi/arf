use super::super::support::{PROMPT, Terminal, run_case};
use super::session_metadata_path;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::fs;

/// Send one raw JSON-RPC request over the platform IPC transport and return the
/// parsed response. Unix sockets carry the request in an HTTP envelope; Windows
/// named pipes carry the bare JSON body.
pub(super) fn raw_json_rpc(socket_path: &str, method: &str, params: Value) -> Result<Value> {
    let body = serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    }))?;
    let response = raw_transport(socket_path, &body)
        .with_context(|| format!("raw JSON-RPC `{method}` request"))?;
    let json = response
        .split_once("\r\n\r\n")
        .map_or(response.as_str(), |(_, json)| json);
    serde_json::from_str(json).with_context(|| format!("parse raw `{method}` response: {json}"))
}

#[cfg(unix)]
fn raw_transport(socket_path: &str, body: &str) -> Result<String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = UnixStream::connect(socket_path).context("connect to IPC socket")?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        stream,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

#[cfg(windows)]
fn raw_transport(socket_path: &str, body: &str) -> Result<String> {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create tokio runtime")?;
    runtime.block_on(async {
        let mut pipe = ClientOptions::new()
            .open(socket_path)
            .context("connect to IPC named pipe")?;
        pipe.write_all(body.as_bytes()).await?;
        pipe.flush().await?;
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), pipe.read_to_end(&mut response))
            .await
            .context("raw IPC request timed out")??;
        Ok(String::from_utf8_lossy(&response).into_owned())
    })
}

pub(super) fn session_socket_path(terminal: &Terminal) -> Result<String> {
    let metadata: Value =
        serde_json::from_str(&fs::read_to_string(session_metadata_path(terminal)?)?)?;
    metadata["socket_path"]
        .as_str()
        .map(str::to_owned)
        .context("session metadata lacks socket_path")
}

/// The `arf ipc` client reshapes responses, so this test pins the raw envelope
/// that other JSON-RPC clients see: one evaluate case per outcome on a single
/// session, then the `shutdown` method that only headless mode accepts.
#[test]
fn raw_json_rpc_responses_have_the_documented_shape() -> Result<()> {
    run_case(
        "ipc-raw-json-rpc",
        &["--with-ipc", "--ipc-eval-unrestricted"],
        |terminal| {
            let socket_path = session_socket_path(terminal)?;
            let result = |error: Value, stdout: &str, value: Value| {
                serde_json::json!({
                    "id": 1,
                    "jsonrpc": "2.0",
                    "result": {
                        "error": error,
                        "stderr": "",
                        "stdout": stdout,
                        "value": value,
                    },
                })
            };
            // Later cases depend on earlier ones: the assignment must persist
            // across requests on the same session.
            let cases = [
                ("value", "1 + 1", result(Value::Null, "", "[1] 2".into())),
                (
                    "stdout",
                    r"cat('RAW_STDOUT\n')",
                    result(Value::Null, "RAW_STDOUT\n", Value::Null),
                ),
                (
                    "error",
                    r"stop('RAW_ERROR')",
                    result("RAW_ERROR".into(), "", Value::Null),
                ),
                (
                    "mixed stdout and value",
                    r"cat('RAW_MIXED\n'); 42",
                    result(Value::Null, "RAW_MIXED\n", "[1] 42".into()),
                ),
                (
                    "assignment",
                    "raw_x <- 123",
                    result(Value::Null, "", Value::Null),
                ),
                (
                    "sees the earlier assignment",
                    "raw_x + 1",
                    result(Value::Null, "", "[1] 124".into()),
                ),
            ];
            for (case, code, mut expected) in cases {
                let response = raw_json_rpc(
                    &socket_path,
                    "evaluate",
                    serde_json::json!({ "code": code }),
                )
                .with_context(|| format!("raw evaluate case `{case}`"))?;
                ensure!(
                    response["result"]["execution_id"].as_u64().is_some(),
                    "raw evaluate case `{case}` lacks a numeric execution ID: {response}"
                );
                expected["result"]["execution_id"] = response["result"]["execution_id"].clone();
                ensure!(
                    response == expected,
                    "raw evaluate case `{case}` has the wrong response: {response}"
                );
            }

            let response = raw_json_rpc(&socket_path, "shutdown", serde_json::json!({}))
                .context("raw shutdown request")?;
            ensure!(
                response["error"]["code"] == -32601,
                "shutdown should be METHOD_NOT_FOUND in REPL mode: {response}"
            );
            ensure!(
                response.get("result").is_none(),
                "rejected shutdown must not carry a result: {response}"
            );
            // The rejected shutdown must leave the REPL running.
            terminal.submit("1 + 2", "[1] 3", PROMPT)
        },
    )
}
