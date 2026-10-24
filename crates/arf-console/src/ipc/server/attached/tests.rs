//! Tests of long-lived connections, driven through `handle_connection` over
//! in-memory streams with a fake main thread.

use super::*;
use crate::ipc::events::{EventBus, ExecutionId, Limits, QueueLimits, Topic};
use crate::ipc::protocol::{
    IpcRequest, IpcResponse, METHOD_NOT_FOUND, PARSE_ERROR, UserInputResult,
};
use crate::ipc::server::connection::{ConnectionContext, handle_connection};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::task::JoinHandle;

const WAIT: Duration = Duration::from_secs(5);

/// Large enough that nothing the tests write ever blocks.
const ROOMY: usize = 1 << 20;

/// Small enough that any write blocks until the client reads.
const TINY: usize = 8;

/// The client end of a connection.
struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl Client {
    fn new(stream: DuplexStream) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }

    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(data).await?;
        self.writer.flush().await
    }

    async fn send_line(&mut self, line: &str) {
        self.send_raw(format!("{line}\n").as_bytes()).await.unwrap();
    }

    async fn send_request(&mut self, id: i64, method: &str, params: Value) {
        self.send_line(&request_line(id, method, params)).await;
    }

    /// The next line, or `None` at end of stream.
    async fn recv_line(&mut self) -> Option<String> {
        let mut line = String::new();
        let n = tokio::time::timeout(WAIT, self.reader.read_line(&mut line))
            .await
            .expect("timed out waiting for a line")
            .unwrap();
        (n > 0).then_some(line)
    }

    async fn recv(&mut self) -> Value {
        let line = self.recv_line().await.expect("connection was closed");
        assert!(line.ends_with('\n'), "{line:?}");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }

    /// Whether anything arrives within a short while.
    async fn is_silent(&mut self) -> bool {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_millis(100), self.reader.read_line(&mut line))
            .await
            .is_err()
    }

    async fn assert_closed(&mut self) {
        assert_eq!(
            self.recv_line().await,
            None,
            "expected the connection to close"
        );
    }

    /// Everything until the server closes the connection.
    async fn read_to_end(&mut self) -> String {
        let mut rest = String::new();
        tokio::time::timeout(WAIT, self.reader.read_to_string(&mut rest))
            .await
            .expect("timed out waiting for the connection to close")
            .unwrap();
        rest
    }

    /// Attach and return the `result` of the response.
    async fn attach(&mut self, params: Value) -> Value {
        self.send_request(0, "attach", params).await;
        let mut response = self.recv().await;
        assert_eq!(response["id"], 0, "{response}");
        response["result"].take()
    }
}

fn request_line(id: i64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn user_input(id: i64) -> String {
    request_line(id, "user_input", json!({"code": "1"}))
}

struct Fixture {
    client: Client,
    bus: EventBus,
    /// Requests that reached the (fake) main thread.
    requests: tokio::sync::mpsc::UnboundedReceiver<IpcRequest>,
    task: JoinHandle<std::io::Result<()>>,
    shutdowns: Arc<AtomicUsize>,
    cancel: CancellationToken,
}

impl Fixture {
    fn new() -> Self {
        Self::with(Limits::default(), ROOMY)
    }

    /// Start serving a connection over a stream that buffers `capacity` bytes
    /// in each direction.
    fn with(limits: Limits, capacity: usize) -> Self {
        let (client, server) = tokio::io::duplex(capacity);
        let (bus, dispatcher) = EventBus::new(limits);
        tokio::spawn(dispatcher.run());

        // The main thread is a plain thread, as in the real server; it only
        // forwards requests so that tests decide how to answer them.
        let (tx, rx) = std::sync::mpsc::channel::<IpcRequest>();
        let (forward, requests) = tokio::sync::mpsc::unbounded_channel();
        std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                if forward.send(request).is_err() {
                    break;
                }
            }
        });

        let shutdowns = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let mut ctx = ConnectionContext::new(tx, bus.clone(), cancel.clone());
        let counter = Arc::clone(&shutdowns);
        ctx.request_shutdown = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let task = tokio::spawn(handle_connection(server, ctx));

        Self {
            client: Client::new(client),
            bus,
            requests,
            task,
            shutdowns,
            cancel,
        }
    }

    async fn next_request(&mut self) -> IpcRequest {
        tokio::time::timeout(WAIT, self.requests.recv())
            .await
            .expect("timed out waiting for a request to reach the main thread")
            .expect("main thread channel closed")
    }

    async fn finished(self) -> std::io::Result<()> {
        tokio::time::timeout(WAIT, self.task)
            .await
            .expect("timed out waiting for the connection handler to finish")
            .unwrap()
    }

    fn publish(&self, topic: &str, data: Value) -> u64 {
        self.bus
            .publish(Topic::internal(topic).unwrap(), data, None)
            .unwrap()
    }
}

fn accepted() -> IpcResponse {
    IpcResponse::UserInput(UserInputResult {
        accepted: true,
        execution_id: None,
    })
}

/// Wait until `condition` holds, letting other tasks run in between.
async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition was not reached in time");
}

fn error_code(response: &Value) -> i64 {
    response["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("not an error response: {response}"))
}

#[tokio::test]
async fn attach_result_describes_the_connection_and_session() {
    let mut f = Fixture::new();
    let result = f
        .client
        .attach(json!({"client": {"name": "test", "version": "0.1"}}))
        .await;

    assert!(result["connection_id"].as_u64().is_some(), "{result}");
    assert_eq!(result["api_version"], 1);
    assert_eq!(
        result["capabilities"],
        json!(["events", "execution_events"])
    );
    let session_id = result["session_id"].as_str().unwrap();
    assert_eq!(session_id.len(), 32);
    assert_eq!(result["session"]["session_id"], session_id);
    assert_eq!(result["session"]["pid"], std::process::id());
    // Attaching never waits for R.
    assert!(result["session"]["r"].is_null());
    assert!(result["session"]["r_unavailable_reason"].is_string());
    assert!(f.requests.try_recv().is_err());
    assert_eq!(f.bus.connection_count(), 1);
}

#[tokio::test]
async fn attach_accepts_missing_params_and_explicit_protocol_one() {
    let mut f = Fixture::new();
    f.client
        .send_line(r#"{"jsonrpc":"2.0","id":0,"method":"attach"}"#)
        .await;
    assert_eq!(f.client.recv().await["result"]["api_version"], 1);

    let mut f = Fixture::new();
    let result = f.client.attach(json!({"protocol": 1})).await;
    assert_eq!(result["api_version"], 1);
}

// Protects the process-global `IN_ALTERNATE_MODE` atomic, which decides
// whether requests reach the main thread.
#[tokio::test]
#[serial_test::serial]
async fn request_sent_in_the_same_write_as_attach_is_answered() {
    let mut f = Fixture::new();
    let both = format!(
        "{}\n{}\n",
        request_line(0, "attach", json!({})),
        user_input(2)
    );
    f.client.send_raw(both.as_bytes()).await.unwrap();

    assert_eq!(f.client.recv().await["id"], 0);
    f.next_request().await.reply.send(accepted()).ok().unwrap();
    let response = f.client.recv().await;
    assert_eq!(response["id"], 2);
    assert_eq!(response["result"]["accepted"], true);
}

#[tokio::test]
async fn events_follow_the_attach_response_in_order() {
    let mut f = Fixture::with(Limits::default(), TINY);
    f.client
        .send_request(0, "attach", json!({"subscribe": ["demo.*"]}))
        .await;
    // The attach response cannot be written yet, so the connection is
    // registered and subscribed while the client has read nothing.
    wait_for(|| f.bus.connection_count() == 1).await;

    let first = f.publish("demo.a", json!({"n": 1}));
    f.publish("other.b", json!({"n": 2}));
    f.bus
        .publish(
            Topic::internal("demo.c").unwrap(),
            json!({"n": 3}),
            Some(ExecutionId(7)),
        )
        .unwrap();

    let attached = f.client.recv().await;
    assert_eq!(attached["id"], 0);
    assert!(attached["result"].is_object());

    let event = f.client.recv().await;
    assert_eq!(event["jsonrpc"], "2.0");
    assert_eq!(event["method"], "event");
    assert!(event.get("id").is_none(), "{event}");
    assert_eq!(event["params"]["seq"], first);
    assert_eq!(event["params"]["topic"], "demo.a");
    assert_eq!(event["params"]["data"], json!({"n": 1}));
    assert!(event["params"]["execution"].is_null());

    let event = f.client.recv().await;
    assert_eq!(event["params"]["topic"], "demo.c");
    assert_eq!(event["params"]["execution"], 7);
    assert!(event["params"]["seq"].as_u64().unwrap() > first);
    assert!(f.client.is_silent().await, "other.b must not be delivered");
}

#[tokio::test]
async fn subscriptions_are_applied_in_arrival_order() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;

    let batch = [
        request_line(1, "subscribe", json!({"patterns": ["a.*", "b.*", "a.*"]})),
        request_line(2, "unsubscribe", json!({"patterns": ["a.*"]})),
        request_line(3, "subscribe", json!({"patterns": ["a.*", "c.d"]})),
        request_line(4, "unsubscribe", json!({"patterns": ["never.there"]})),
    ]
    .join("\n");
    f.client.send_line(&batch).await;

    for (id, key, count) in [
        (1, "added", 2),
        (2, "removed", 1),
        (3, "added", 2),
        (4, "removed", 0),
    ] {
        let response = f.client.recv().await;
        assert_eq!(response["id"], id, "{response}");
        assert_eq!(response["result"][key], count, "{response}");
    }

    f.publish("a.x", json!(1));
    f.publish("c.d", json!(2));
    f.publish("z.z", json!(3));
    f.publish("b.y", json!(4));
    for topic in ["a.x", "c.d", "b.y"] {
        assert_eq!(f.client.recv().await["params"]["topic"], topic);
    }
}

#[tokio::test]
async fn unsubscribed_topics_stop_arriving() {
    let mut f = Fixture::new();
    f.client
        .attach(json!({"subscribe": ["keep.*", "drop.*"]}))
        .await;
    f.client
        .send_request(1, "unsubscribe", json!({"patterns": ["drop.*"]}))
        .await;
    assert_eq!(f.client.recv().await["result"]["removed"], 1);

    f.publish("drop.x", json!(1));
    f.publish("keep.x", json!(2));
    assert_eq!(f.client.recv().await["params"]["topic"], "keep.x");
}

#[tokio::test]
async fn an_invalid_pattern_applies_none_of_the_request() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;

    f.client
        .send_request(
            1,
            "subscribe",
            json!({"patterns": ["fine.topic", "not valid"]}),
        )
        .await;
    let response = f.client.recv().await;
    assert_eq!(error_code(&response), i64::from(INVALID_PARAMS));
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not valid"),
        "{response}"
    );

    // Had the valid pattern been applied, it would be removed here.
    f.client
        .send_request(2, "unsubscribe", json!({"patterns": ["fine.topic"]}))
        .await;
    assert_eq!(f.client.recv().await["result"]["removed"], 0);
}

#[tokio::test]
async fn exceeding_the_subscription_limit_applies_none_of_the_request() {
    let limits = Limits {
        max_subscriptions_per_connection: 3,
        ..Limits::default()
    };
    let mut f = Fixture::with(limits, ROOMY);
    f.client.attach(json!({"subscribe": ["a.1", "a.2"]})).await;

    f.client
        .send_request(1, "subscribe", json!({"patterns": ["a.3", "a.4"]}))
        .await;
    assert_eq!(
        error_code(&f.client.recv().await),
        i64::from(INVALID_PARAMS)
    );

    // Neither new pattern was added: one more fits, and the other is absent.
    f.client
        .send_request(2, "subscribe", json!({"patterns": ["a.3"]}))
        .await;
    assert_eq!(f.client.recv().await["result"]["added"], 1);
    f.client
        .send_request(3, "unsubscribe", json!({"patterns": ["a.4"]}))
        .await;
    assert_eq!(f.client.recv().await["result"]["removed"], 0);
}

#[tokio::test]
async fn attach_with_bad_subscriptions_is_refused_and_closes() {
    let limits = Limits {
        max_subscriptions_per_connection: 2,
        ..Limits::default()
    };
    for subscribe in [
        json!(["fine", "not valid"]),
        json!(["a", "b", "c"]),
        json!("not a list"),
    ] {
        let mut f = Fixture::with(limits, ROOMY);
        f.client
            .send_request(0, "attach", json!({"subscribe": subscribe}))
            .await;
        let response = f.client.recv().await;
        assert_eq!(
            error_code(&response),
            i64::from(INVALID_PARAMS),
            "{response}"
        );
        f.client.assert_closed().await;
        assert_eq!(f.bus.connection_count(), 0);
        f.finished().await.unwrap();
    }
}

#[tokio::test]
async fn unsupported_protocol_is_refused_and_closes() {
    let mut f = Fixture::new();
    f.client
        .send_request(0, "attach", json!({"protocol": 2}))
        .await;
    let response = f.client.recv().await;
    assert_eq!(error_code(&response), i64::from(INVALID_PARAMS));
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("protocol"),
        "{response}"
    );
    f.client.assert_closed().await;
    assert_eq!(f.bus.connection_count(), 0);
    f.finished().await.unwrap();
}

#[tokio::test]
async fn attach_over_http_is_rejected() {
    let mut f = Fixture::new();
    let body = request_line(1, "attach", json!({}));
    let http = format!(
        "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}\n",
        body.len() + 1
    );
    f.client.send_raw(http.as_bytes()).await.unwrap();

    let written = f.client.read_to_end().await;
    let (head, json) = written.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{written}");
    let response: Value = serde_json::from_str(json).unwrap();
    assert_eq!(error_code(&response), i64::from(INVALID_REQUEST));
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("attach requires"),
        "{response}"
    );
    assert_eq!(f.bus.connection_count(), 0);
}

#[tokio::test]
async fn attach_without_an_id_closes_silently() {
    let mut f = Fixture::new();
    f.client
        .send_line(r#"{"jsonrpc":"2.0","method":"attach"}"#)
        .await;
    assert_eq!(f.client.read_to_end().await, "");
    assert_eq!(f.bus.connection_count(), 0);
}

#[tokio::test]
async fn attach_waits_for_the_end_of_its_line() {
    let mut f = Fixture::new();
    let line = request_line(0, "attach", json!({}));
    f.client.send_raw(line.as_bytes()).await.unwrap();
    assert!(f.client.is_silent().await, "answered before the line ended");
    f.client.send_raw(b" \t").await.unwrap();
    assert!(f.client.is_silent().await);
    f.client.send_raw(b"\r\n").await.unwrap();

    let response = f.client.recv().await;
    assert_eq!(response["result"]["api_version"], 1);
}

#[tokio::test]
async fn attach_not_ending_its_line_is_an_ordinary_single_shot_request() {
    // Other bytes after the request on the same line.
    let mut f = Fixture::new();
    let line = request_line(1, "attach", json!({}));
    f.client
        .send_raw(format!("{line} {{}}\n").as_bytes())
        .await
        .unwrap();
    let written = f.client.read_to_end().await;
    let (head, json) = written.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{written}");
    let response: Value = serde_json::from_str(json).unwrap();
    assert_eq!(error_code(&response), i64::from(METHOD_NOT_FOUND));
    assert_eq!(f.bus.connection_count(), 0);

    // The stream ends before the line does.
    let mut f = Fixture::new();
    f.client.send_raw(line.as_bytes()).await.unwrap();
    f.client.writer.shutdown().await.unwrap();
    let written = f.client.read_to_end().await;
    assert!(written.starts_with("HTTP/1.1 200 OK"), "{written}");
    assert!(written.contains("Method not found"), "{written}");
}

#[tokio::test]
async fn a_second_attach_is_refused_and_the_connection_stays_open() {
    let mut f = Fixture::new();
    f.client.attach(json!({"subscribe": ["a.*"]})).await;

    f.client
        .send_request(1, "attach", json!({"subscribe": ["b.*"]}))
        .await;
    let response = f.client.recv().await;
    assert_eq!(response["id"], 1);
    assert_eq!(error_code(&response), i64::from(INVALID_REQUEST));

    // Still attached, with the original subscriptions only.
    f.publish("b.x", json!(1));
    f.publish("a.x", json!(2));
    assert_eq!(f.client.recv().await["params"]["topic"], "a.x");
}

#[tokio::test]
async fn a_line_over_the_limit_closes_the_connection() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    // No newline: the limit has to be enforced while the line is still being
    // read. The write fails once the server has closed the connection.
    let long = vec![b'a'; MAX_REQUEST_BYTES + 1];
    let _ = f.client.send_raw(&long).await;
    f.client.assert_closed().await;
    f.finished().await.unwrap();
}

#[tokio::test]
async fn a_complete_line_over_the_limit_closes_the_connection() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    let mut long = vec![b'a'; MAX_REQUEST_BYTES + 1];
    long.push(b'\n');
    let _ = f.client.send_raw(&long).await;
    f.client.assert_closed().await;
}

#[tokio::test]
async fn a_line_exactly_at_the_limit_is_accepted() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    let head = r#"{"jsonrpc":"2.0","id":9,"method":"nope","params":""#;
    let tail = r#""}"#;
    let padding = "a".repeat(MAX_REQUEST_BYTES - head.len() - tail.len());
    let line = format!("{head}{padding}{tail}");
    assert_eq!(line.len(), MAX_REQUEST_BYTES);
    f.client.send_line(&line).await;

    let response = f.client.recv().await;
    assert_eq!(response["id"], 9);
    assert_eq!(error_code(&response), i64::from(METHOD_NOT_FOUND));
}

#[tokio::test]
async fn a_line_split_over_many_writes_is_reassembled() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    let line = request_line(5, "nope", json!({"pad": "x".repeat(10_000)}));
    for piece in line.as_bytes().chunks(777) {
        f.client.send_raw(piece).await.unwrap();
    }
    f.client.send_raw(b"\n").await.unwrap();
    assert_eq!(f.client.recv().await["id"], 5);
}

#[tokio::test]
async fn invalid_json_closes_the_connection() {
    for bad in ["this is not json", r#"{"a":1} {"a":2}"#, "{\"jsonrpc\":"] {
        let mut f = Fixture::new();
        f.client.attach(json!({})).await;
        f.client.send_line(bad).await;
        f.client.assert_closed().await;
        f.finished().await.unwrap();
    }
}

#[tokio::test]
async fn malformed_requests_get_error_responses() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;

    f.client
        .send_line(r#"{"jsonrpc":"1.0","id":5,"method":"x"}"#)
        .await;
    let response = f.client.recv().await;
    assert_eq!(response["id"], 5);
    assert_eq!(error_code(&response), i64::from(INVALID_REQUEST));

    f.client.send_line(r#"{"jsonrpc":"2.0","id":6}"#).await;
    let response = f.client.recv().await;
    assert_eq!(response["id"], 6);
    assert_eq!(error_code(&response), i64::from(INVALID_REQUEST));

    for not_a_request in ["[]", r#""text""#, "42"] {
        f.client.send_line(not_a_request).await;
        let response = f.client.recv().await;
        assert!(response.get("id").is_none(), "{response}");
        assert_eq!(error_code(&response), i64::from(INVALID_REQUEST));
    }

    f.client.send_request(7, "no_such_method", json!({})).await;
    let response = f.client.recv().await;
    assert_eq!(response["id"], 7);
    assert_eq!(error_code(&response), i64::from(METHOD_NOT_FOUND));

    f.client
        .send_request(8, "subscribe", json!({"patterns": 5}))
        .await;
    assert_eq!(
        error_code(&f.client.recv().await),
        i64::from(INVALID_PARAMS)
    );
}

#[tokio::test]
async fn notifications_and_blank_lines_get_no_response() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;

    let notification =
        json!({"jsonrpc": "2.0", "method": "subscribe", "params": {"patterns": ["a.*"]}});
    f.client.send_raw(b"\n  \n\r\n").await.unwrap();
    f.client.send_line(&notification.to_string()).await;
    f.client.send_request(3, "no_such_method", json!({})).await;

    // The first thing back is the answer to the request, not to the
    // notification or the blank lines.
    let response = f.client.recv().await;
    assert_eq!(response["id"], 3);
    assert!(f.client.is_silent().await);

    // The notification was ignored, as for single-shot requests.
    f.publish("a.x", json!(1));
    assert!(f.client.is_silent().await);
}

#[tokio::test]
async fn windows_line_endings_are_accepted() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    let line = request_line(4, "no_such_method", json!({}));
    f.client
        .send_raw(format!("{line}\r\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(f.client.recv().await["id"], 4);
}

#[tokio::test]
#[serial_test::serial]
async fn requests_run_concurrently_and_complete_in_any_order() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    f.client.send_line(&user_input(1)).await;
    f.client.send_line(&user_input(2)).await;

    let first = f.next_request().await;
    let second = f.next_request().await;
    second.reply.send(accepted()).ok().unwrap();
    assert_eq!(f.client.recv().await["id"], 2);
    first.reply.send(accepted()).ok().unwrap();
    assert_eq!(f.client.recv().await["id"], 1);
}

#[tokio::test]
#[serial_test::serial]
async fn requests_are_answered_with_the_same_errors_as_single_shot() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    f.client
        .send_request(1, "user_input", json!({"code": "a\u{0}b"}))
        .await;
    assert_eq!(
        error_code(&f.client.recv().await),
        i64::from(INVALID_PARAMS)
    );
    f.client.send_request(2, "user_input", json!({})).await;
    assert_eq!(
        error_code(&f.client.recv().await),
        i64::from(INVALID_PARAMS)
    );
    assert!(f.requests.try_recv().is_err());
}

#[tokio::test]
#[serial_test::serial]
async fn the_request_beyond_the_outstanding_limit_gets_r_busy() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;

    let burst: Vec<String> = (1..=33).map(user_input).collect();
    f.client.send_line(&burst.join("\n")).await;

    let refused = f.client.recv().await;
    assert_eq!(refused["id"], 33);
    assert_eq!(error_code(&refused), i64::from(R_BUSY));
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("outstanding"),
        "{refused}"
    );

    let mut held = Vec::new();
    for _ in 0..MAX_OUTSTANDING_REQUESTS {
        held.push(f.next_request().await);
    }
    assert!(
        f.requests.try_recv().is_err(),
        "the refused request was spawned"
    );

    // Finishing one makes room for another.
    held.pop().unwrap().reply.send(accepted()).ok().unwrap();
    let response = f.client.recv().await;
    assert_eq!(response["result"]["accepted"], true);
    f.client.send_line(&user_input(34)).await;
    f.next_request().await.reply.send(accepted()).ok().unwrap();
    assert_eq!(f.client.recv().await["id"], 34);
}

#[tokio::test]
#[serial_test::serial]
async fn closing_the_connection_cancels_requests_the_main_thread_has_not_started() {
    let mut f = Fixture::new();
    f.client.attach(json!({})).await;
    f.client.send_line(&user_input(1)).await;
    f.client.send_line(&user_input(2)).await;
    let first = f.next_request().await;
    let second = f.next_request().await;
    assert!(!first.reply.is_closed() && !second.reply.is_closed());

    let Fixture { client, task, .. } = f;
    drop(client);

    // The main thread checks this before starting a request.
    for mut request in [first, second] {
        tokio::time::timeout(WAIT, request.reply.closed())
            .await
            .expect("the request was not cancelled");
    }
    tokio::time::timeout(WAIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn stopping_the_server_closes_the_connection_and_cancels_requests() {
    let mut f = Fixture::new();
    f.client.attach(json!({"subscribe": ["a.*"]})).await;
    f.client.send_line(&user_input(1)).await;
    let mut request = f.next_request().await;

    f.cancel.cancel();
    f.client.assert_closed().await;
    tokio::time::timeout(WAIT, request.reply.closed())
        .await
        .expect("the request was not cancelled");
    wait_for(|| f.bus.connection_count() == 0).await;
}

#[tokio::test]
async fn closing_the_connection_removes_it_from_the_bus() {
    let mut f = Fixture::new();
    f.client.attach(json!({"subscribe": ["a.*"]})).await;
    assert_eq!(f.bus.connection_count(), 1);

    let Fixture { client, bus, .. } = f;
    drop(client);
    wait_for(|| bus.connection_count() == 0).await;
}

#[tokio::test(start_paused = true)]
async fn a_client_that_never_reads_the_attach_response_is_disconnected() {
    let mut f = Fixture::with(Limits::default(), TINY);
    f.client.send_request(0, "attach", json!({})).await;

    // Time advances by itself while everything waits on the stalled write.
    let error = f.task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn a_client_that_stops_reading_events_is_disconnected() {
    let mut f = Fixture::with(Limits::default(), ROOMY);
    f.client.attach(json!({"subscribe": ["a.*"]})).await;
    // Fill the stream so that the next write cannot complete.
    let filler = "x".repeat(ROOMY / 2);
    f.publish("a.fill", json!(filler));
    f.publish("a.more", json!(filler));
    f.publish("a.stuck", json!(filler));

    let error = f.task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(f.bus.connection_count(), 0);
}

#[tokio::test]
async fn overflowing_the_event_queue_closes_the_connection() {
    let limits = Limits {
        connection_queue: QueueLimits {
            max_events: 2,
            max_bytes: 1024 * 1024,
        },
        ..Limits::default()
    };
    let mut f = Fixture::with(limits, TINY);
    f.client
        .send_request(0, "attach", json!({"subscribe": ["flood.*"]}))
        .await;
    assert_eq!(f.client.recv().await["id"], 0);

    // The first event cannot be written because the client reads nothing, so
    // the queue behind it overflows.
    for n in 0..20 {
        f.publish("flood.x", json!(n));
        tokio::task::yield_now().await;
    }
    wait_for(|| f.bus.connection_count() == 0).await;

    // The connection is cut, possibly in the middle of a line; the lines
    // that did arrive are whole events, and fewer than were published.
    let written = f.client.read_to_end().await;
    let complete = written.rsplit_once('\n').map_or("", |(whole, _)| whole);
    let lines: Vec<&str> = complete.lines().collect();
    assert!(lines.len() < 20, "{written}");
    for line in lines {
        assert!(line.contains(r#""method":"event""#), "{line}");
    }
    tokio::time::timeout(WAIT, f.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
}

#[tokio::test]
async fn a_steady_stream_of_events_does_not_hold_back_responses() {
    let mut f = Fixture::new();
    f.client.attach(json!({"subscribe": ["busy.*"]})).await;

    let stop = Arc::new(AtomicBool::new(false));
    let publisher = {
        let bus = f.bus.clone();
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let _ = bus.publish(Topic::internal("busy.x").unwrap(), json!(0), None);
                for _ in 0..3 {
                    tokio::task::yield_now().await;
                }
            }
        })
    };
    for id in 1..=20 {
        f.client.send_request(id, "no_such_method", json!({})).await;
    }

    let mut answered = 0;
    while answered < 20 {
        if f.client.recv().await["method"] != "event" {
            answered += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    publisher.await.unwrap();
}

/// Make `shutdown` acceptable. The flag itself is never raised: tests count
/// calls to the injected shutdown hook instead.
fn enable_shutdown_method() {
    crate::ipc::set_headless_shutdown(Arc::new(AtomicBool::new(false)));
}

#[tokio::test]
async fn shutdown_is_carried_out_only_after_its_response_is_written() {
    enable_shutdown_method();
    let mut f = Fixture::with(Limits::default(), TINY);
    f.client.send_request(0, "attach", json!({})).await;
    // The tiny stream makes every write wait for the client to read.
    assert_eq!(f.client.recv().await["id"], 0);

    f.client.send_request(1, "shutdown", json!({})).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        f.shutdowns.load(Ordering::SeqCst),
        0,
        "shutdown was triggered before the response was written"
    );

    let response = f.client.recv().await;
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["accepted"], true);
    wait_for(|| f.shutdowns.load(Ordering::SeqCst) == 1).await;

    // Ending the connection does not carry the shutdown out a second time.
    let shutdowns = Arc::clone(&f.shutdowns);
    let Fixture { client, task, .. } = f;
    drop(client);
    tokio::time::timeout(WAIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_is_not_held_back_by_a_client_that_does_not_read() {
    enable_shutdown_method();
    let mut f = Fixture::with(Limits::default(), TINY);
    f.client.send_request(0, "attach", json!({})).await;
    assert_eq!(f.client.recv().await["id"], 0);

    f.client.send_request(1, "shutdown", json!({})).await;
    let error = f.task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(f.shutdowns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_parse_error_on_the_first_frame_is_still_a_single_shot_response() {
    let mut f = Fixture::new();
    f.client.send_raw(b"}").await.unwrap();
    let written = f.client.read_to_end().await;
    assert!(written.starts_with("HTTP/1.1 200 OK"), "{written}");
    assert!(written.contains(&PARSE_ERROR.to_string()), "{written}");
}

#[tokio::test]
async fn a_long_line_received_in_tiny_pieces_is_reassembled_in_linear_time() {
    let (bus, _dispatcher) = EventBus::new(Limits::default());
    let (tx, _rx) = std::sync::mpsc::channel();
    let ctx = ConnectionContext::new(tx, bus.clone(), CancellationToken::new());
    let cancel = CancellationToken::new();
    let shutdown = Arc::new(ShutdownLatch::new(Arc::clone(&ctx.request_shutdown)));
    let (responses, mut received) = mpsc::channel(RESPONSE_QUEUE_CAPACITY);
    let mut lines = RequestLoop {
        line: Vec::new(),
        requests: JoinSet::new(),
        responses,
        connection: bus.register_connection().id(),
        ctx: &ctx,
        shutdown: &shutdown,
        cancel: &cancel,
    };

    let head = r#"{"jsonrpc":"2.0","id":1,"method":"nope","params":""#;
    let padding = "a".repeat(512 * 1024);
    let line = format!("{head}{padding}\"}}\n");

    // A rescan of the buffered bytes on every piece would take minutes.
    let started = std::time::Instant::now();
    for byte in line.as_bytes() {
        assert!(lines.feed(std::slice::from_ref(byte)).await);
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "reading a line byte by byte took {:?}",
        started.elapsed()
    );

    let outgoing = lines
        .requests
        .join_next()
        .await
        .expect("the line did not start a request")
        .unwrap();
    assert!(received.try_recv().is_err());
    assert_eq!(outgoing.response.id, Some(json!(1)));
    assert_eq!(
        outgoing.response.error.map(|error| error.code),
        Some(METHOD_NOT_FOUND)
    );
}

#[tokio::test(start_paused = true)]
async fn an_accepted_shutdown_is_carried_out_when_an_earlier_write_stalls() {
    enable_shutdown_method();
    let mut f = Fixture::with(Limits::default(), TINY);
    f.client
        .send_request(0, "attach", json!({"subscribe": ["a.*"]}))
        .await;
    assert_eq!(f.client.recv().await["id"], 0);

    // The writer stalls on this event, so the shutdown response behind it is
    // never written.
    f.publish("a.stuck", json!("x".repeat(100)));
    f.client.send_request(1, "shutdown", json!({})).await;

    let error = f.task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(f.shutdowns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn a_bus_disconnect_cancels_requests_even_while_a_write_is_stalled() {
    let limits = Limits {
        connection_queue: QueueLimits {
            max_events: 2,
            max_bytes: 1024 * 1024,
        },
        ..Limits::default()
    };
    let mut f = Fixture::with(limits, TINY);
    f.client
        .send_request(0, "attach", json!({"subscribe": ["flood.*"]}))
        .await;
    assert_eq!(f.client.recv().await["id"], 0);
    f.client.send_line(&user_input(1)).await;
    f.client.send_line(&user_input(2)).await;
    let mut first = f.next_request().await;
    let mut second = f.next_request().await;

    // The client reads nothing, so the writer stalls on the first event and
    // the queue behind it overflows.
    for n in 0..20 {
        f.publish("flood.x", json!(n));
        tokio::task::yield_now().await;
    }
    wait_for(|| f.bus.connection_count() == 0).await;

    // Far sooner than the write timeout.
    for request in [&mut first, &mut second] {
        tokio::time::timeout(Duration::from_secs(5), request.reply.closed())
            .await
            .expect("the request was not cancelled after the bus disconnected the connection");
    }
}

#[tokio::test]
async fn a_bus_disconnect_ends_a_stalled_attach_response_write() {
    let limits = Limits {
        connection_queue: QueueLimits {
            max_events: 2,
            max_bytes: 1024 * 1024,
        },
        ..Limits::default()
    };
    let f = Fixture::with(limits, TINY);
    let Fixture {
        mut client,
        bus,
        task,
        ..
    } = f;
    client
        .send_request(0, "attach", json!({"subscribe": ["flood.*"]}))
        .await;
    wait_for(|| bus.connection_count() == 1).await;
    for n in 0..20 {
        bus.publish(Topic::internal("flood.x").unwrap(), json!(n), None)
            .unwrap();
        tokio::task::yield_now().await;
    }
    wait_for(|| bus.connection_count() == 0).await;

    // The client has read nothing; the write of the attach response is still
    // stalled and would otherwise last until the write timeout.
    let error = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the stalled write was not cut short")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
}

/// An `attach` request line of exactly `len` bytes, without the newline.
fn attach_line_of_len(len: usize) -> String {
    let head = r#"{"jsonrpc":"2.0","id":1,"method":"attach","params":{"client":{"name":""#;
    let tail = r#""}}}"#;
    let line = format!("{head}{}{tail}", "a".repeat(len - head.len() - tail.len()));
    assert_eq!(line.len(), len);
    line
}

#[tokio::test]
async fn an_attach_line_at_the_limit_is_accepted() {
    let mut f = Fixture::new();
    let line = attach_line_of_len(MAX_REQUEST_BYTES);
    f.client.send_line(&line).await;
    assert_eq!(f.client.recv().await["result"]["api_version"], 1);
}

#[tokio::test]
async fn an_attach_line_made_too_long_by_trailing_whitespace_closes_the_connection() {
    for trailing in [&b" \n"[..], b"\t\r\n"] {
        let mut f = Fixture::new();
        let mut data = attach_line_of_len(MAX_REQUEST_BYTES).into_bytes();
        data.extend_from_slice(trailing);
        f.client.send_raw(&data).await.unwrap();

        // Closed without a response, not served as a single-shot request.
        assert_eq!(f.client.read_to_end().await, "");
        assert_eq!(f.bus.connection_count(), 0);
        f.finished().await.unwrap();
    }
}

#[tokio::test]
async fn an_attach_line_made_too_long_by_whitespace_sent_later_closes_the_connection() {
    let mut f = Fixture::new();
    let line = attach_line_of_len(MAX_REQUEST_BYTES);
    f.client.send_raw(line.as_bytes()).await.unwrap();
    f.client.send_raw(b" ").await.unwrap();
    f.client.send_raw(b" \n").await.unwrap();
    assert_eq!(f.client.read_to_end().await, "");
    assert_eq!(f.bus.connection_count(), 0);
}
