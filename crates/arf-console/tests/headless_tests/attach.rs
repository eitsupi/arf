//! Long-lived (`attach`) connections against a real headless session.

use super::support::{HeadlessProcess, parse_ipc_json};
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
        let reader = &mut self.reader;
        let mut line = String::new();
        let read = self
            .runtime
            .block_on(async {
                tokio::time::timeout(REPLY_TIMEOUT, reader.read_line(&mut line)).await
            })
            .expect("timed out waiting for a reply")
            .expect("read from the IPC connection");
        assert_ne!(read, 0, "the IPC connection closed before a reply");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }
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
    assert_eq!(result["capabilities"], json!(["events"]), "{attached}");
    assert!(result["connection_id"].is_u64(), "{attached}");
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
    let mut client = attach_subscribed(&socket_path_of(&process), &["demo.*"]);

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
    while event.is_none() || response.is_none() {
        let message = client.recv();
        if message["method"] == "event" {
            event = Some(message);
        } else {
            response = Some(message);
        }
    }
    let (event, response) = (event.unwrap(), response.unwrap());
    assert_eq!(response["id"], 2, "{response}");
    assert_eq!(response["result"]["value"], "[1] TRUE", "{response}");
    assert_eq!(response["result"]["error"], Value::Null, "{response}");
    let params = &event["params"];
    assert_eq!(params["topic"], "demo.hello", "{event}");
    assert_eq!(
        params["data"],
        json!({"n": 1, "tags": ["a", null], "ok": true}),
        "{event}"
    );
    assert_eq!(params["execution"], Value::Null, "{event}");
    assert!(params["seq"].is_u64(), "{event}");
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
