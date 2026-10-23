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
