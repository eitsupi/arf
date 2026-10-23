//! Long-lived (`attach`) connections against a real headless session.

use super::support::{HeadlessProcess, parse_ipc_json};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::time::Duration;

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// A newline-delimited JSON-RPC client over the session's socket or pipe.
struct AttachedClient {
    writer: Box<dyn Write>,
    lines: mpsc::Receiver<std::io::Result<String>>,
}

impl AttachedClient {
    fn connect(socket_path: &str) -> Self {
        #[cfg(unix)]
        let (reader, writer) = {
            let stream = std::os::unix::net::UnixStream::connect(socket_path)
                .expect("connect to the IPC socket");
            (stream.try_clone().expect("clone the stream"), stream)
        };
        #[cfg(windows)]
        let (reader, writer) = {
            let pipe = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(socket_path)
                .expect("open the IPC pipe");
            (pipe.try_clone().expect("clone the pipe"), pipe)
        };

        // Reads block without a timeout, so a thread feeds a channel that
        // the test can wait on with one.
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let failed = line.is_err();
                if tx.send(line).is_err() || failed {
                    break;
                }
            }
        });
        Self {
            writer: Box::new(writer),
            lines,
        }
    }

    fn send(&mut self, value: &Value) {
        let mut line = value.to_string();
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.flush())
            .expect("write to the IPC connection");
    }

    fn recv(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(REPLY_TIMEOUT)
            .expect("timed out waiting for a reply")
            .expect("read from the IPC connection");
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
