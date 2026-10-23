//! End-to-end tests for R handlers exposed to IPC clients.

use super::support::{HeadlessProcess, parse_ipc_json};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::runtime::Runtime;

const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(unix)]
async fn connect(path: &str) -> Stream {
    Stream::connect(path).await.expect("connect to IPC socket")
}

#[cfg(windows)]
async fn connect(path: &str) -> Stream {
    use tokio::net::windows::named_pipe::ClientOptions;

    const ERROR_PIPE_BUSY: i32 = 231;
    let deadline = tokio::time::Instant::now() + IO_TIMEOUT;
    loop {
        match ClientOptions::new().open(path) {
            Ok(pipe) => return pipe,
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => panic!("open the IPC pipe: {error}"),
        }
    }
}

struct Client {
    runtime: Runtime,
    reader: BufReader<ReadHalf<Stream>>,
    writer: WriteHalf<Stream>,
}

impl Client {
    fn connect(path: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create Tokio runtime");
        let stream = runtime.block_on(connect(path));
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
                tokio::time::timeout(IO_TIMEOUT, async {
                    writer.write_all(line.as_bytes()).await?;
                    writer.flush().await
                })
                .await
            })
            .expect("timed out writing to IPC")
            .expect("write to IPC");
    }

    fn recv(&mut self) -> Value {
        let reader = &mut self.reader;
        let mut line = String::new();
        let bytes = self
            .runtime
            .block_on(async { tokio::time::timeout(IO_TIMEOUT, reader.read_line(&mut line)).await })
            .expect("timed out waiting for an IPC response")
            .expect("read from IPC");
        assert_ne!(bytes, 0, "IPC connection closed before its response");
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line:?}"))
    }

    fn attach(path: &str) -> Self {
        let mut client = Self::connect(path);
        client.send(&request(
            1,
            "attach",
            json!({"client": {"name": "handler-test"}}),
        ));
        let response = client.recv();
        assert!(response["result"].is_object(), "{response}");
        assert!(
            response["result"]["capabilities"]
                .as_array()
                .is_some_and(|capabilities| capabilities.iter().any(|value| value == "handlers")),
            "locked handler API must be advertised: {response}"
        );
        client
    }
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn session_socket_path(process: &HeadlessProcess) -> String {
    let response = process.ipc_session().expect("query session");
    assert!(response.success, "{}", response.stderr);
    parse_ipc_json(&response)["socket_path"]
        .as_str()
        .expect("session socket_path")
        .to_owned()
}

#[cfg(unix)]
fn single_shot_call(path: &str, request: Value) -> Value {
    use tokio::io::AsyncReadExt;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create Tokio runtime");
    let response = runtime.block_on(async {
        let mut stream = Stream::connect(path).await.expect("connect to IPC socket");
        let body = request.to_string();
        let http_request = format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::time::timeout(IO_TIMEOUT, async {
            stream.write_all(http_request.as_bytes()).await?;
            stream.flush().await?;
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await?;
            std::io::Result::Ok(response)
        })
        .await
        .expect("timed out waiting for a single-shot IPC response")
        .expect("read single-shot IPC response")
    });
    let response = String::from_utf8_lossy(&response);
    let body = response
        .split_once("\r\n\r\n")
        .map_or(response.as_ref(), |(_, body)| body);
    serde_json::from_str(body).unwrap_or_else(|error| panic!("{error}: {body:?}"))
}

#[cfg(windows)]
fn single_shot_call(path: &str, request: Value) -> Value {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create Tokio runtime");
    let response = runtime.block_on(async {
        let mut stream = connect(path).await;
        let mut line = request.to_string();
        line.push('\n');
        tokio::time::timeout(IO_TIMEOUT, async {
            stream.write_all(line.as_bytes()).await?;
            stream.flush().await?;
            let mut reader = BufReader::new(stream);
            let mut response = String::new();
            reader.read_line(&mut response).await?;
            std::io::Result::Ok(response)
        })
        .await
        .expect("timed out waiting for a single-shot IPC response")
        .expect("read single-shot IPC response")
    });
    serde_json::from_str(&response).expect("parse single-shot response")
}

fn exposed(expression: &str) -> HeadlessProcess {
    HeadlessProcess::spawn_with_args(&["--ipc-expose", expression])
        .unwrap_or_else(|error| panic!("headless startup with exposed handlers failed: {error}"))
}

#[test]
fn exposed_handler_call_captures_output_and_returns_json() {
    let process = exposed(
        r#"list("demo/add" = function(args) {
  invisible(gc())
  cat("handler stdout")
  message("handler stderr")
  list(sum = sum(unlist(args$values)))
})"#,
    );
    let path = session_socket_path(&process);

    let response = single_shot_call(
        &path,
        request(
            10,
            "call",
            json!({"handler": "demo/add", "args": {"values": [4, 6]}}),
        ),
    );
    assert_eq!(response["id"], 10, "{response}");
    assert_eq!(
        response["result"]["value"],
        json!({"sum": 10.0}),
        "{response}"
    );

    let mut client = Client::attach(&path);

    client.send(&request(
        2,
        "call",
        json!({"handler": "demo/add", "args": {"values": [2, 3]}}),
    ));
    let response = client.recv();
    assert_eq!(response["id"], 2, "{response}");
    assert_eq!(
        response["result"]["value"],
        json!({"sum": 5.0}),
        "{response}"
    );
    assert_eq!(response["result"]["stdout"], "handler stdout", "{response}");
    assert!(
        response["result"]["stderr"]
            .as_str()
            .unwrap()
            .contains("handler stderr"),
        "{response}"
    );

    client.send(&request(3, "session", json!({})));
    let session = client.recv();
    assert_eq!(
        session["result"]["handler_api"]["state"], "locked",
        "{session}"
    );
    assert_eq!(
        session["result"]["handler_api"]["handlers"],
        json!(["demo/add"]),
        "{session}"
    );

    let history = process.ipc_history(&[]).expect("query session history");
    assert!(history.success, "{}", history.stderr);
    let history = parse_ipc_json(&history);
    assert_eq!(
        history["entries"],
        json!([]),
        "handler calls must not enter history"
    );
}

#[test]
fn unexposed_handler_api_is_reported_and_cannot_be_called() {
    let process = HeadlessProcess::spawn().expect("start headless without exposed handlers");
    let path = session_socket_path(&process);
    let session = single_shot_call(&path, request(1, "session", json!({})));
    assert_eq!(
        session["result"]["handler_api"]["state"], "unexposed",
        "{session}"
    );
    assert_eq!(
        session["result"]["handler_api"]["handlers"],
        json!([]),
        "{session}"
    );

    let rejected = single_shot_call(
        &path,
        request(2, "call", json!({"handler": "demo/missing"})),
    );
    assert_eq!(rejected["error"]["code"], -32010, "{rejected}");
}

#[test]
fn call_errors_are_specific_and_do_not_break_the_session() {
    let process = exposed(
        r#"list(
  "demo/error" = function(args) {
    cat("before error")
    message("error detail")
    stop("handler failed")
  },
  "demo/return" = function(args) function() NULL
)"#,
    );
    let mut client = Client::attach(&session_socket_path(&process));

    for (id, handler, expected_code) in [
        (2, "demo/missing", -32010),
        (3, "demo/error", -32014),
        (4, "demo/return", -32015),
    ] {
        client.send(&request(id, "call", json!({"handler": handler})));
        let response = client.recv();
        assert_eq!(response["id"], id, "{response}");
        assert_eq!(response["error"]["code"], expected_code, "{response}");
        if handler == "demo/error" {
            assert!(
                response["error"]["data"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("handler failed"),
                "{response}"
            );
            assert_eq!(
                response["error"]["data"]["stdout"], "before error",
                "{response}"
            );
            assert!(
                response["error"]["data"]["stderr"]
                    .as_str()
                    .unwrap()
                    .contains("error detail"),
                "{response}"
            );
        }
    }

    client.send(&request(
        6,
        "call",
        json!({"handler": "demo/error", "args": 9_007_199_254_740_993_u64}),
    ));
    let response = client.recv();
    assert_eq!(response["error"]["code"], -32013, "{response}");

    client.send(&request(5, "call", json!({"handler": "demo/error"})));
    let response = client.recv();
    assert_eq!(response["error"]["code"], -32014, "{response}");
}

#[test]
fn handler_calls_work_with_a_restricted_evaluate_policy() {
    let process = HeadlessProcess::spawn_with_args(&[
        "--ipc-expose",
        r#"list("demo/answer" = function(args) 42L)"#,
        "--ipc-eval-allow-function",
        "+",
    ])
    .expect("start headless with a restricted evaluate policy");
    let mut client = Client::attach(&session_socket_path(&process));

    client.send(&request(
        2,
        "evaluate",
        json!({"code": "Sys.getenv('HOME')"}),
    ));
    let rejected = client.recv();
    assert_eq!(rejected["error"]["code"], -32005, "{rejected}");

    client.send(&request(3, "call", json!({"handler": "demo/answer"})));
    let called = client.recv();
    assert_eq!(called["id"], 3, "{called}");
    assert_eq!(called["result"]["value"], 42);
}

#[test]
fn expired_queued_call_is_not_run() {
    let process = exposed(
        r#"list("demo/mark" = function(args) {
  assign("handler_timeout_marker", TRUE, envir = .GlobalEnv)
  NULL
})"#,
    );
    let mut client = Client::attach(&session_socket_path(&process));

    client.send(&request(
        2,
        "evaluate",
        json!({"code": "Sys.sleep(0.25); TRUE"}),
    ));
    client.send(&request(
        3,
        "call",
        json!({"handler": "demo/mark", "queue_timeout_ms": 1}),
    ));

    let mut evaluate_response = None;
    let mut call_response = None;
    while evaluate_response.is_none() || call_response.is_none() {
        let response = client.recv();
        match response["id"].as_i64() {
            Some(2) => evaluate_response = Some(response),
            Some(3) => call_response = Some(response),
            _ => panic!("unexpected response: {response}"),
        }
    }
    assert!(evaluate_response.unwrap()["error"].is_null());
    assert_eq!(call_response.unwrap()["error"]["code"], -32012);

    client.send(&request(
        4,
        "evaluate",
        json!({"code": "exists('handler_timeout_marker', .GlobalEnv, inherits = FALSE)"}),
    ));
    let response = client.recv();
    assert_eq!(response["id"], 4, "{response}");
    assert_eq!(response["result"]["value"], "[1] FALSE", "{response}");
}

#[test]
fn accepted_calls_and_evaluations_run_in_fifo_order() {
    let process = exposed(
        r#"handler_fifo <- character()
list(
  "demo/append" = function(args) {
    handler_fifo <<- c(handler_fifo, args$value)
    NULL
  },
  "demo/read" = function(args) as.list(handler_fifo)
)"#,
    );
    let mut client = Client::attach(&session_socket_path(&process));

    client.send(&request(
        2,
        "call",
        json!({"handler": "demo/append", "args": {"value": "A"}}),
    ));
    client.send(&request(
        3,
        "evaluate",
        json!({"code": "handler_fifo <- c(handler_fifo, 'B'); handler_fifo"}),
    ));
    client.send(&request(
        4,
        "call",
        json!({"handler": "demo/append", "args": {"value": "C"}}),
    ));
    client.send(&request(5, "call", json!({"handler": "demo/read"})));

    let mut responses = std::collections::HashMap::new();
    for _ in 0..4 {
        let response = client.recv();
        let id = response["id"].as_i64().expect("response id");
        assert!(
            responses.insert(id, response).is_none(),
            "duplicate response id"
        );
    }
    for id in [2, 3, 4, 5] {
        let response = &responses[&id];
        assert!(response["error"].is_null(), "{response}");
    }
    assert_eq!(responses[&5]["result"]["value"], json!(["A", "B", "C"]));
}

#[test]
fn disconnect_drops_a_call_that_has_not_started() {
    let process = exposed(
        r#"list("demo/mark" = function(args) {
  assign("handler_disconnect_marker", TRUE, envir = .GlobalEnv)
  NULL
})"#,
    );
    let path = session_socket_path(&process);
    let mut client = Client::attach(&path);

    client.send(&request(
        2,
        "evaluate",
        json!({"code": "Sys.sleep(0.25); TRUE"}),
    ));
    client.send(&request(3, "call", json!({"handler": "demo/mark"})));
    drop(client);

    let response = single_shot_call(
        &path,
        request(
            4,
            "evaluate",
            json!({
                "code": "exists('handler_disconnect_marker', .GlobalEnv, inherits = FALSE)"
            }),
        ),
    );
    assert_eq!(response["id"], 4, "{response}");
    assert_eq!(response["result"]["value"], "[1] FALSE", "{response}");
}

#[test]
fn invalid_exposure_aborts_headless_startup_with_a_diagnostic() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["headless", "--ipc-expose", "list('bad' = 1)"])
        .output()
        .expect("run arf headless");
    assert!(!output.status.success(), "startup unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("handler") || stderr.contains("function"),
        "{stderr}"
    );
}
