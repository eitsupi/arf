//! Long-lived (`attach`) connections against a real headless session.

use super::support::{HeadlessProcess, parse_ipc_json, run_ipc_command};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::runtime::Runtime;

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// Open the session's socket or pipe.
#[cfg(unix)]
async fn open_stream(socket_path: &str) -> Stream {
    Stream::connect(socket_path)
        .await
        .expect("connect to the IPC socket")
}

/// Open the session's pipe, waiting while every server instance is busy.
#[cfg(windows)]
async fn open_stream(socket_path: &str) -> Stream {
    use tokio::net::windows::named_pipe::ClientOptions;

    // `ERROR_PIPE_BUSY`: all instances of the pipe are connected to other
    // clients, so the server has not yet created the next one.
    const ERROR_PIPE_BUSY: i32 = 231;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match ClientOptions::new().open(socket_path) {
            Ok(pipe) => return pipe,
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("open the IPC pipe: {error}"),
        }
    }
}

/// A newline-delimited JSON-RPC client over the session's socket or pipe.
///
/// Reads and writes are asynchronous and time-limited, so a connection that
/// stops making progress fails the test instead of hanging it. One buffered
/// reader serves the whole connection so that bytes read ahead of a line are
/// never lost.
struct AttachedClient {
    runtime: Runtime,
    reader: BufReader<ReadHalf<Stream>>,
    writer: WriteHalf<Stream>,
    connection_id: Option<u64>,
}

impl AttachedClient {
    fn connect(socket_path: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create a tokio runtime");
        let stream = runtime.block_on(open_stream(socket_path));
        let (reader, writer) = tokio::io::split(stream);
        Self {
            runtime,
            reader: BufReader::new(reader),
            writer,
            connection_id: None,
        }
    }

    fn send(&mut self, value: &Value) {
        let mut line = value.to_string();
        line.push('\n');
        let writer = &mut self.writer;
        self.runtime
            .block_on(async {
                tokio::time::timeout(WRITE_TIMEOUT, async {
                    writer.write_all(line.as_bytes()).await?;
                    writer.flush().await
                })
                .await
            })
            .expect("timed out writing to the IPC connection")
            .expect("write to the IPC connection");
    }

    fn recv(&mut self) -> Value {
        self.recv_timeout(REPLY_TIMEOUT)
            .expect("timed out waiting for a reply")
    }

    /// Evaluate `code` and wait for both its response and its completion
    /// event, so no stray completion reaches later assertions.
    fn evaluate_to_completion(&mut self, id: i64, code: &str) -> Value {
        self.send(&request(id, "evaluate", json!({"code": code})));
        let mut response: Option<Value> = None;
        let mut completed = false;
        while response.is_none() || !completed {
            let message = self.recv();
            if message["id"] == id {
                response = Some(message);
            } else if message["params"]["topic"] == "arf.execution_completed" {
                completed = true;
            }
        }
        response.expect("evaluation response")
    }

    fn evaluate_with_completion(&mut self, id: i64, code: &str, visible: bool) -> (Value, Value) {
        self.send(&request(
            id,
            "evaluate",
            json!({"code": code, "visible": visible}),
        ));
        let mut response: Option<Value> = None;
        let mut completions = Vec::new();
        loop {
            if let Some(result) = &response
                && let Some(event) = completions.iter().find(|event: &&Value| {
                    event["params"]["data"]["id"] == result["result"]["execution_id"]
                })
            {
                return (result.clone(), (*event).clone());
            }
            let message = self.recv();
            if message["id"] == id {
                response = Some(message);
            } else if message["params"]["topic"] == "arf.execution_completed" {
                completions.push(message);
            }
        }
    }

    fn recv_timeout(&mut self, timeout: Duration) -> Option<Value> {
        let reader = &mut self.reader;
        let mut line = String::new();
        let read = self
            .runtime
            .block_on(async { tokio::time::timeout(timeout, reader.read_line(&mut line)).await })
            .ok()?;
        let read = read.expect("read from the IPC connection");
        assert_ne!(read, 0, "the IPC connection closed before a reply");
        Some(serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}")))
    }
}

#[test]
fn headless_evaluate_status_matches_completion() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut client = attach_subscribed(&socket_path_of(&process), &["arf.execution_completed"]);
    let cases = [
        ("1 + 1", false, "ok"),
        ("stop('HEADLESS_ATTACH_ERROR')", false, "error"),
        (
            "1 + 1; stop('HEADLESS_ATTACH_MIDDLE_ERROR'); 3",
            false,
            "error",
        ),
        ("1 + 1", true, "ok"),
        ("stop('HEADLESS_ATTACH_VISIBLE_ERROR')", true, "error"),
        (
            "1 + 1; stop('HEADLESS_ATTACH_VISIBLE_MIDDLE_ERROR'); 3",
            true,
            "error",
        ),
    ];
    for (id, (code, visible, expected_status)) in (2..).zip(cases) {
        let (response, completion) = client.evaluate_with_completion(id, code, visible);
        assert!(response["error"].is_null(), "{response}");
        assert_eq!(response["result"]["status"], expected_status, "{response}");
        assert_eq!(
            completion["params"]["data"]["status"], expected_status,
            "{completion}"
        );
        assert_eq!(
            completion["params"]["data"]["id"], response["result"]["execution_id"],
            "response and completion differ: {response}; {completion}"
        );
    }
}

#[test]
fn headless_handler_call_does_not_emit_a_user_code_completion() {
    let process = HeadlessProcess::spawn_with_args(&[
        "--ipc-expose",
        r#"list("demo/read" = function(args) {
            .Call("arf_publish_event", "demo.handler", NULL, PACKAGE = "(embedding)")
            if (isTRUE(args$fail)) stop("handler failure")
            list(value = 42L)
        })"#,
    ])
    .expect("headless process with exposed handler should start");
    let mut client = attach_subscribed(
        &socket_path_of(&process),
        &["demo.handler", "arf.execution_completed"],
    );

    client.send(&request(
        2,
        "call",
        json!({"handler": "demo/read", "args": {}}),
    ));
    let first = client.recv();
    let second = client.recv();
    let (response, event) = if first["id"] == 2 {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(response["id"], 2, "{response}");
    assert_eq!(response["result"]["value"], json!({"value": 42}));
    assert!(
        response["result"]["execution_id"].as_u64().is_some(),
        "handler call response should include an execution ID: {response}"
    );
    assert_eq!(event["params"]["topic"], "demo.handler");
    assert_eq!(
        event["params"]["execution"],
        response["result"]["execution_id"]
    );
    assert_eq!(
        client.recv_timeout(Duration::from_millis(250)),
        None,
        "handler calls must not emit completion events"
    );

    client.send(&request(
        3,
        "call",
        json!({"handler": "demo/read", "args": {"fail": true}}),
    ));
    let first = client.recv();
    let second = client.recv();
    let (response, event) = if first["id"] == 3 {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(response["error"]["code"], -32014, "{response}");
    let execution_id = response["error"]["data"]["execution_id"]
        .as_u64()
        .expect("failed handler execution ID");
    assert_eq!(event["params"]["topic"], "demo.handler");
    assert_eq!(event["params"]["execution"], execution_id);
    assert_eq!(client.recv_timeout(Duration::from_millis(250)), None);
}

#[test]
fn headless_evaluate_timeout_keeps_execution_context_until_completion() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut client = attach_subscribed(
        &socket_path_of(&process),
        &["demo.timeout", "arf.execution_completed"],
    );
    for visible in [false, true] {
        client.send(&request(
            1,
            "evaluate",
            json!({"code": "1", "visible": visible, "timeout_ms": 0}),
        ));
        let rejected = client.recv();
        assert_eq!(rejected["id"], 1, "{rejected}");
        assert_eq!(rejected["error"]["code"], -32602, "{rejected}");

        // Allow R to start even while the workspace tests run in parallel,
        // then keep it busy beyond the reply deadline.
        client.send(&request(
            2,
            "evaluate",
            json!({
                "code": format!(
                    "{}; Sys.sleep(2)",
                    publish_call("demo.timeout", "NULL")
                ),
                "timeout_ms": 1000,
                "visible": visible,
            }),
        ));

        let published = client.recv();
        let timed_out = client.recv();
        let completion = client.recv();
        assert_eq!(published["params"]["topic"], "demo.timeout", "{published}");
        assert_eq!(timed_out["id"], 2, "{timed_out}");
        assert_eq!(timed_out["error"]["code"], -32007, "{timed_out}");
        assert_eq!(
            timed_out["error"]["data"]["timeout_ms"], 1000,
            "{timed_out}"
        );
        let execution_id = timed_out["error"]["data"]["execution_id"]
            .as_u64()
            .expect("timeout response should preserve the started execution ID");
        assert_eq!(published["params"]["execution"], json!(execution_id));
        assert_eq!(
            completion["params"]["topic"], "arf.execution_completed",
            "{completion}"
        );
        assert_eq!(completion["params"]["data"]["id"], json!(execution_id));
        assert_eq!(completion["params"]["data"]["status"], "ok");
        assert!(
            published["params"]["seq"].as_u64() < completion["params"]["seq"].as_u64(),
            "published event must precede completion: published={published}, completion={completion}"
        );
        assert_eq!(
            client.recv_timeout(Duration::from_millis(250)),
            None,
            "reply timeout must not complete an execution early or more than once"
        );
    }
}

#[test]
fn headless_interrupt_emits_interrupted_completion_distinct_from_error() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let mut client = attach_subscribed(
        &socket_path_of(&process),
        &["demo.interrupt", "arf.execution_completed"],
    );
    let mut next_id = 100;
    for visible in [false, true] {
        let code = format!(
            "kept_value <- 42L; {}; Sys.sleep(30)",
            publish_call("demo.interrupt", "NULL")
        );
        let id = next_id;
        next_id += 1;
        client.send(&request(
            id,
            "evaluate",
            json!({"code": code, "visible": visible, "timeout_ms": 30000}),
        ));
        let started = client.recv();
        assert_eq!(started["params"]["topic"], "demo.interrupt", "{started}");
        interrupt_headless(&process);

        let mut evaluation = None;
        let mut completion = None;
        while evaluation.is_none() || completion.is_none() {
            let message = client.recv();
            if message["id"] == id {
                evaluation = Some(message);
            } else if message["params"]["topic"] == "arf.execution_completed" {
                completion = Some(message);
            }
        }
        let evaluation = evaluation.expect("evaluation response");
        let completion = completion.expect("completion event");
        assert!(
            evaluation["result"]["error"]
                .as_str()
                .is_some_and(|error| error.to_ascii_lowercase().contains("interrupt")),
            "evaluation response should report interruption (visible={visible}): {evaluation}"
        );
        assert_eq!(
            evaluation["result"]["status"], "interrupted",
            "visible={visible}: {evaluation}"
        );
        assert_eq!(
            completion["params"]["data"]["status"], "interrupted",
            "visible={visible}: {completion}"
        );
        assert_eq!(completion["params"]["data"]["visible"], visible);
        let execution_id = completion["params"]["data"]["id"]
            .as_u64()
            .expect("completion execution ID");
        assert_eq!(evaluation["result"]["execution_id"], execution_id);

        let id = next_id;
        next_id += 1;
        let kept = client.evaluate_to_completion(id, "kept_value");
        assert_eq!(kept["result"]["value"], "[1] 42", "{kept}");
    }

    // `user_input` acknowledges acceptance first; the interrupt then ends the
    // visible REPL evaluation and the remaining lines of the input never run.
    let id = next_id;
    next_id += 1;
    let send_code = format!(
        "send_kept_value <- 42L; {}; Sys.sleep(30)\nsend_remainder <- TRUE",
        publish_call("demo.interrupt", "NULL")
    );
    client.send(&request(id, "user_input", json!({"code": send_code})));
    let mut accepted = None;
    let mut started = None;
    let mut completion = None;
    let mut interrupted_requested = false;
    while completion.is_none() {
        let message = client.recv();
        if message["id"] == id {
            accepted = Some(message);
        } else if message["params"]["topic"] == "demo.interrupt" {
            started = Some(message);
        } else if message["params"]["topic"] == "arf.execution_completed" {
            completion = Some(message);
        }
        if started.is_some() && !interrupted_requested {
            interrupted_requested = true;
            interrupt_headless(&process);
        }
    }
    let accepted = accepted.expect("user_input acknowledgement");
    let completion = completion.expect("send completion event");
    assert_eq!(
        completion["params"]["data"]["status"], "interrupted",
        "{completion}"
    );
    assert_eq!(
        completion["params"]["data"]["id"], accepted["result"]["execution_id"],
        "{completion}"
    );
    let id = next_id;
    let kept = client.evaluate_to_completion(
        id,
        "c(send_kept_value, exists('send_remainder', envir = .GlobalEnv))",
    );
    assert_eq!(kept["result"]["value"], "[1] 42  0", "{kept}");

    client.send(&request(
        3,
        "evaluate",
        json!({"code": r#"stop('ordinary completion error')"#}),
    ));
    let mut error_response = None;
    let mut error_completion = None;
    while error_response.is_none() || error_completion.is_none() {
        let message = client.recv();
        if message["id"] == 3 {
            error_response = Some(message);
        } else if message["params"]["topic"] == "arf.execution_completed" {
            error_completion = Some(message);
        }
    }
    let error_response = error_response.expect("ordinary error response");
    let error_completion = error_completion.expect("ordinary error completion");
    assert!(
        error_response["result"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("ordinary completion error")),
        "ordinary evaluation error was lost: {error_response}"
    );
    assert_eq!(
        error_completion["params"]["data"]["status"], "error",
        "ordinary errors must not be reported as interrupted: {error_completion}"
    );

    let suspended_code = format!(
        r#"tryCatch({{ {}; Sys.sleep(30) }}, interrupt = function(e) stop('ordinary error after handled interrupt'))"#,
        publish_call("demo.interrupt", "NULL")
    );
    client.send(&request(
        4,
        "evaluate",
        json!({"code": suspended_code, "timeout_ms": 30000}),
    ));
    let started = client.recv();
    assert_eq!(started["params"]["topic"], "demo.interrupt", "{started}");
    let output = run_ipc_command(&["ipc", "interrupt", "--pid", &process.pid.to_string()]);
    assert!(
        output.status.success(),
        "interrupt request failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let accepted: Value = serde_json::from_slice(&output.stdout).expect("interrupt JSON");
    assert_eq!(accepted["accepted"], true, "{accepted}");
    let mut suspended_response = None;
    let mut suspended_completion = None;
    while suspended_response.is_none() || suspended_completion.is_none() {
        let message = client.recv();
        if message["id"] == 4 {
            suspended_response = Some(message);
        } else if message["params"]["topic"] == "arf.execution_completed" {
            suspended_completion = Some(message);
        }
    }
    let suspended_response = suspended_response.expect("suspended error response");
    let suspended_completion = suspended_completion.expect("suspended error completion");
    assert!(
        suspended_response["result"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("ordinary error after handled interrupt")),
        "ordinary error after handled interrupt was misreported: {suspended_response}"
    );
    assert_eq!(
        suspended_completion["params"]["data"]["status"], "error",
        "pending interrupt must not relabel an ordinary R error: {suspended_completion}"
    );
}

fn interrupt_headless(process: &HeadlessProcess) {
    let output = run_ipc_command(&["ipc", "interrupt", "--pid", &process.pid.to_string()]);
    assert!(
        output.status.success(),
        "interrupt request failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let accepted: Value = serde_json::from_slice(&output.stdout).expect("interrupt JSON");
    assert_eq!(accepted["accepted"], true, "{accepted}");
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

#[test]
fn test_attach_serves_several_requests_on_one_connection() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");
    let session = process.ipc_session().expect("session should run");
    assert!(session.success, "{}", session.stderr);
    let session = parse_ipc_json(&session);
    let socket_path = session["socket_path"].as_str().expect("socket_path");

    let mut client = AttachedClient::connect(socket_path);
    client.send(&request(
        1,
        "attach",
        json!({"client": {"name": "integration-test"}, "subscribe": ["demo.*"]}),
    ));
    let attached = client.recv();
    assert_eq!(attached["id"], 1, "{attached}");
    let result = &attached["result"];
    assert_eq!(result["api_version"], 1, "{attached}");
    assert_eq!(
        result["capabilities"],
        json!(["events", "execution_events"]),
        "{attached}"
    );
    assert!(result["connection_id"].is_u64(), "{attached}");
    client.connection_id = result["connection_id"].as_u64();
    // The attach result and the `session` method agree on the process.
    assert_eq!(result["session_id"], session["session_id"], "{attached}");
    assert_eq!(result["session"]["pid"], process.pid, "{attached}");

    // Several requests on the same connection.
    client.send(&request(2, "evaluate", json!({"code": "1 + 1"})));
    let evaluated = client.recv();
    assert_eq!(evaluated["id"], 2, "{evaluated}");
    assert_eq!(evaluated["result"]["value"], "[1] 2", "{evaluated}");

    client.send(&request(3, "evaluate", json!({"code": "40 + 2"})));
    let evaluated = client.recv();
    assert_eq!(evaluated["id"], 3, "{evaluated}");
    assert_eq!(evaluated["result"]["value"], "[1] 42", "{evaluated}");

    client.send(&request(4, "subscribe", json!({"patterns": ["other.*"]})));
    let subscribed = client.recv();
    assert_eq!(subscribed["result"]["added"], 1, "{subscribed}");

    // Ordinary single-shot clients keep working while a connection is attached.
    let eval = process.ipc_eval("2 + 2").expect("eval should run");
    assert!(eval.success, "{}", eval.stderr);
    assert_eq!(parse_ipc_json(&eval)["value"], "[1] 4");
}

/// Publish `topic` with `data_expr` (an R expression) and return `TRUE` or `FALSE`.
fn publish_call(topic: &str, data_expr: &str) -> String {
    format!(r#".Call("arf_publish_event", "{topic}", {data_expr}, PACKAGE = "(embedding)")"#)
}

/// Attach with `patterns` subscribed and require the attach to succeed.
fn attach_subscribed(socket_path: &str, patterns: &[&str]) -> AttachedClient {
    let mut client = AttachedClient::connect(socket_path);
    client.send(&request(
        1,
        "attach",
        json!({"client": {"name": "integration-test"}, "subscribe": patterns}),
    ));
    let attached = client.recv();
    assert_eq!(attached["id"], 1, "{attached}");
    assert!(attached["result"].is_object(), "{attached}");
    client.connection_id = attached["result"]["connection_id"].as_u64();
    client
}

fn socket_path_of(process: &HeadlessProcess) -> String {
    let session = process.ipc_session().expect("session should run");
    assert!(session.success, "{}", session.stderr);
    parse_ipc_json(&session)["socket_path"]
        .as_str()
        .expect("socket_path")
        .to_owned()
}

#[test]
fn test_r_code_publishes_events_to_subscribed_connections() {
    // The help viewer is not involved in headless sessions, so this also shows
    // that the routines are registered without it.
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");
    let mut client = attach_subscribed(
        &socket_path_of(&process),
        &["demo.*", "arf.execution_completed"],
    );

    client.send(&request(
        2,
        "evaluate",
        json!({"code": r#"
stopifnot(is.loaded("arf_api_version", PACKAGE = "(embedding)", type = ".Call"))
stopifnot(identical(.Call("arf_api_version", PACKAGE = "(embedding)"), 1L))
.Call("arf_publish_event", "demo.hello", list(n = 1L, tags = list("a", NULL), ok = TRUE),
      PACKAGE = "(embedding)")
"#}),
    ));

    // The event and the response travel separately, so either may come first.
    let mut event = None;
    let mut response = None;
    let mut completion = None;
    while event.is_none() || response.is_none() || completion.is_none() {
        let message = client.recv();
        if message["method"] == "event" {
            if message["params"]["topic"] == "demo.hello" {
                event = Some(message);
            } else if message["params"]["topic"] == "arf.execution_completed" {
                completion = Some(message);
            }
        } else {
            response = Some(message);
        }
    }
    let (event, response, completion) = (event.unwrap(), response.unwrap(), completion.unwrap());
    assert_eq!(response["id"], 2, "{response}");
    assert_eq!(response["result"]["value"], "[1] TRUE", "{response}");
    assert_eq!(response["result"]["error"], Value::Null, "{response}");
    let execution_id = response["result"]["execution_id"]
        .as_u64()
        .expect("evaluation response should include its execution ID");
    let params = &event["params"];
    assert_eq!(params["topic"], "demo.hello", "{event}");
    assert_eq!(
        params["data"],
        json!({"n": 1, "tags": ["a", null], "ok": true}),
        "{event}"
    );
    assert_eq!(params["execution"], json!(execution_id), "{event}");
    assert!(params["seq"].is_u64(), "{event}");

    let completion_params = &completion["params"];
    assert_eq!(
        completion_params["topic"], "arf.execution_completed",
        "{completion}"
    );
    assert_eq!(completion_params["execution"], json!(execution_id));
    assert_eq!(completion_params["data"]["id"], json!(execution_id));
    assert_eq!(
        completion_params["data"]["origin"],
        attached_connection_id(&client)
    );
    assert_eq!(completion_params["data"]["kind"], "user_code");
    assert_eq!(completion_params["data"]["visible"], false);
    assert_eq!(completion_params["data"]["status"], "ok");
    assert!(
        params["seq"].as_u64() < completion_params["seq"].as_u64(),
        "execution notification must precede completion: event={event}, completion={completion}"
    );

    // `user_input` acknowledges acceptance before the R code finishes. Its
    // completion is still emitted exactly once after the request has run.
    client.send(&request(
        3,
        "user_input",
        json!({
            "code": format!("{}; 1 + 2", publish_call("demo.user_input", "NULL"))
        }),
    ));
    let mut accepted = None;
    let mut user_event = None;
    let mut user_completion = None;
    while accepted.is_none() || user_event.is_none() || user_completion.is_none() {
        let message = client.recv();
        if message["method"] == "event" {
            match message["params"]["topic"].as_str() {
                Some("demo.user_input") => user_event = Some(message),
                Some("arf.execution_completed") => user_completion = Some(message),
                _ => {}
            }
        } else {
            accepted = Some(message);
        }
    }
    let accepted = accepted.unwrap();
    let user_event = user_event.unwrap();
    let user_completion = user_completion.unwrap();
    assert_eq!(accepted["id"], 3, "{accepted}");
    assert_eq!(accepted["result"]["accepted"], true, "{accepted}");
    let user_execution_id = accepted["result"]["execution_id"]
        .as_u64()
        .expect("user_input response should include its execution ID");
    assert_eq!(user_event["params"]["execution"], json!(user_execution_id));
    assert_eq!(
        user_completion["params"]["data"]["id"],
        json!(user_execution_id)
    );
    assert_eq!(
        user_completion["params"]["data"]["visible"], true,
        "user_input is visible REPL evaluation"
    );
    assert!(
        user_event["params"]["seq"].as_u64() < user_completion["params"]["seq"].as_u64(),
        "the R notification should precede completion"
    );
    assert_eq!(
        client.recv_timeout(Duration::from_millis(250)),
        None,
        "one input request must produce only one completion event"
    );
}

fn attached_connection_id(client: &AttachedClient) -> Value {
    json!(client.connection_id.expect("attached connection ID"))
}

#[test]
fn headless_evaluation_errors_complete_once_and_recover_with_a_new_id() {
    let history_dir = tempfile::tempdir().expect("create isolated history directory");
    let process = HeadlessProcess::spawn_with_args(&[
        "--history-dir",
        history_dir.path().to_str().expect("history path is UTF-8"),
    ])
    .expect("headless process should start");
    let mut client = attach_subscribed(&socket_path_of(&process), &["arf.execution_completed"]);

    for (error_request_id, visible) in [(2, false), (4, true)] {
        client.send(&request(
            error_request_id,
            "evaluate",
            json!({
                "code": "stop('EXPECTED_EXECUTION_FAILURE')",
                "visible": visible,
            }),
        ));
        let (failed_response, failed_completion) =
            receive_evaluation_completion(&mut client, error_request_id);
        assert!(failed_response["error"].is_null(), "{failed_response}");
        if visible {
            assert!(
                failed_response["result"]["stderr"]
                    .as_str()
                    .is_some_and(|message| message.contains("EXPECTED_EXECUTION_FAILURE")),
                "visible eval should capture the R error: {failed_response}"
            );
        } else {
            assert!(
                failed_response["result"]["error"]
                    .as_str()
                    .is_some_and(|message| message.contains("EXPECTED_EXECUTION_FAILURE")),
                "silent eval should expose the caught R error: {failed_response}"
            );
        }
        let failed_id = failed_response["result"]["execution_id"]
            .as_u64()
            .expect("failed evaluation response should include an execution ID");
        assert_eq!(
            failed_completion["params"]["data"]["id"],
            json!(failed_id),
            "{failed_completion}"
        );
        assert_eq!(failed_completion["params"]["execution"], json!(failed_id));
        assert_eq!(failed_completion["params"]["data"]["visible"], visible);
        assert_eq!(
            failed_completion["params"]["data"]["status"], "error",
            "{failed_completion}"
        );
        assert_eq!(
            client.recv_timeout(Duration::from_millis(250)),
            None,
            "one failed evaluation must emit exactly one completion"
        );

        let success_request_id = error_request_id + 1;
        client.send(&request(
            success_request_id,
            "evaluate",
            json!({"code": "42", "visible": visible}),
        ));
        let (success_response, success_completion) =
            receive_evaluation_completion(&mut client, success_request_id);
        let success_id = success_response["result"]["execution_id"]
            .as_u64()
            .expect("successful evaluation response should include an execution ID");
        assert_ne!(success_id, failed_id);
        assert_eq!(
            success_completion["params"]["data"]["id"],
            json!(success_id)
        );
        assert_eq!(success_completion["params"]["execution"], json!(success_id));
        assert_eq!(success_completion["params"]["data"]["visible"], visible);
        assert_eq!(
            success_completion["params"]["data"]["status"], "ok",
            "{success_completion}"
        );
        assert_eq!(
            client.recv_timeout(Duration::from_millis(250)),
            None,
            "one successful evaluation must emit exactly one completion"
        );
    }

    let history = process.ipc_history(&[]).expect("query session history");
    assert!(history.success, "{}", history.stderr);
    let history = parse_ipc_json(&history);
    let entries = history["entries"]
        .as_array()
        .expect("history entries should be an array");
    for (command, expected_status) in [("stop('EXPECTED_EXECUTION_FAILURE')", 1), ("42", 0)] {
        let matching_entries: Vec<_> = entries
            .iter()
            .filter(|entry| entry["command"].as_str() == Some(command))
            .collect();
        assert_eq!(matching_entries.len(), 2, "history entries for {command:?}");
        assert!(
            matching_entries
                .iter()
                .all(|entry| entry["exit_status"].as_i64() == Some(expected_status)),
            "history exit_status for {command:?} should be {expected_status}: {matching_entries:?}"
        );
    }
}

fn receive_evaluation_completion(client: &mut AttachedClient, request_id: i64) -> (Value, Value) {
    let mut response = None;
    let mut completion = None;
    while response.is_none() || completion.is_none() {
        let message = client.recv();
        if message["method"] == "event" {
            assert_eq!(
                message["params"]["topic"], "arf.execution_completed",
                "{message}"
            );
            assert!(completion.is_none(), "duplicate completion: {message}");
            completion = Some(message);
        } else {
            assert_eq!(message["id"], request_id, "{message}");
            response = Some(message);
        }
    }
    (response.unwrap(), completion.unwrap())
}

#[test]
fn test_r_events_reach_clients_while_r_keeps_computing() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");
    let mut client = attach_subscribed(&socket_path_of(&process), &["demo.*"]);

    // R keeps running until the client has seen the event, so the event can
    // only have been delivered while the evaluation was still in progress.
    let release = tempfile::tempdir().expect("create a temporary directory");
    let release_file = release
        .path()
        .join("received")
        .to_string_lossy()
        .replace('\\', "/");
    client.send(&request(
        2,
        "evaluate",
        json!({"code": format!(
            r#"
{publish}
deadline <- Sys.time() + 25
while (!file.exists("{release_file}") && Sys.time() < deadline) Sys.sleep(0.02)
file.exists("{release_file}")
"#,
            publish = publish_call("demo.early", "NULL")
        )}),
    ));

    let event = client.recv();
    assert_eq!(
        event["method"], "event",
        "the event must come first: {event}"
    );
    assert_eq!(event["params"]["topic"], "demo.early", "{event}");
    assert_eq!(event["params"]["data"], Value::Null, "{event}");

    std::fs::write(release.path().join("received"), "").expect("release R");
    let response = client.recv();
    assert_eq!(response["id"], 2, "{response}");
    assert_eq!(response["result"]["value"], "[1] TRUE", "{response}");
}

#[test]
fn test_r_publishing_errors_are_r_errors_and_leave_the_session_usable() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");
    let mut client = attach_subscribed(&socket_path_of(&process), &["demo.*"]);

    let cases = [
        (publish_call("arf.internal", "NULL"), "reserved"),
        (publish_call("bad topic", "NULL"), "invalid character"),
        (publish_call("demo.ok", "1:2"), "(at JSON Pointer"),
    ];
    for (offset, (code, expected)) in cases.iter().enumerate() {
        let id = 10 + offset as i64;
        client.send(&request(id, "evaluate", json!({"code": code})));
        let response = client.recv();
        assert_eq!(response["id"], id, "{response}");
        let error = response["result"]["error"].as_str().unwrap_or_else(|| {
            panic!("expected an R error for {code}: {response}");
        });
        assert!(error.contains(expected), "{code}: {error}");
    }

    // The session still works and nothing was published by the failed calls.
    client.send(&request(
        20,
        "evaluate",
        json!({"code": publish_call("demo.after", "list(i = 1L)")}),
    ));
    let mut seen_event = None;
    let mut seen_response = None;
    while seen_event.is_none() || seen_response.is_none() {
        let message = client.recv();
        if message["method"] == "event" {
            seen_event = Some(message);
        } else {
            seen_response = Some(message);
        }
    }
    assert_eq!(seen_event.unwrap()["params"]["topic"], "demo.after");
    assert_eq!(seen_response.unwrap()["result"]["value"], "[1] TRUE");
}
