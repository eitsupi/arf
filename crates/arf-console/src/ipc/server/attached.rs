//! Long-lived connections: newline-delimited JSON-RPC in both directions,
//! with requests answered concurrently and events pushed by the server.
//!
//! A connection becomes long-lived when its first request is `attach`. From
//! then on every line in either direction is one JSON value:
//!
//! * The reader (see [`read_requests`]) parses lines, applies `subscribe` and
//!   `unsubscribe` itself in arrival order, and runs every other method as a
//!   task owned by the connection, at most [`MAX_OUTSTANDING_REQUESTS`] at a
//!   time.
//! * The writer (see [`write_outgoing`]) is the only code that writes. It
//!   takes responses from a bounded queue and events from the connection's
//!   [`EventStream`], one line at a time, choosing between the two at random
//!   so neither can starve the other. Every write has a timeout.
//!
//! The two run as halves of one future. Whichever half ends first cancels the
//! other, and the connection closes. Dropping the reader's task set drops the
//! requests it still owns: requests R has not started are cancelled through
//! their closed reply channel, and results of requests already running are
//! discarded.

use super::connection::{
    ATTACH_METHOD, ConnectionContext, MAX_REQUEST_BYTES, READ_CHUNK_BYTES, is_accepted_shutdown,
};
use super::dispatch::{dispatch_request, parse_params};
use crate::ipc::events::{ConnectionId, Event, EventStream, SubscribeError, SubscriptionPattern};
use crate::ipc::protocol::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, JsonRpcRequest, JsonRpcResponse, R_BUSY,
};
use crate::ipc::session_result::{collect_session_result, session_id};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

/// Version of the set of methods and events a client can rely on.
const API_VERSION: u32 = 1;

/// Version of the line protocol. `attach` may ask for it with `protocol`.
const PROTOCOL_VERSION: i64 = 1;

/// Requests (other than `subscribe`, `unsubscribe` and `attach`) that may be
/// outstanding on one connection. Further ones are answered with `R_BUSY`.
const MAX_OUTSTANDING_REQUESTS: usize = 32;

/// Capacity of the queue of responses waiting for the writer. When it is
/// full the reader waits, so a client that does not read its responses
/// eventually stops being read from.
const RESPONSE_QUEUE_CAPACITY: usize = MAX_OUTSTANDING_REQUESTS * 2;

/// Longest a single line may take to write before the connection is closed.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Reported as `r_unavailable_reason` in the session info of the `attach`
/// result, which never waits for R.
const ATTACH_SESSION_REASON: &str = "R was not queried while attaching";

#[derive(Deserialize, Default)]
struct AttachParams {
    /// Informational only; never used to make decisions.
    client: Option<ClientInfo>,
    #[serde(default)]
    subscribe: Vec<String>,
    /// `None` means the first protocol version.
    protocol: Option<i64>,
}

#[derive(Deserialize)]
struct ClientInfo {
    name: Option<String>,
    version: Option<String>,
}

#[derive(Deserialize)]
struct PatternsParams {
    patterns: Vec<String>,
}

/// A response on its way to the writer.
struct Outgoing {
    response: JsonRpcResponse,
    /// The response accepts a `shutdown`, which is carried out once the
    /// response has been written.
    shutdown_accepted: bool,
}

impl Outgoing {
    fn respond(response: JsonRpcResponse) -> Self {
        Self {
            response,
            shutdown_accepted: false,
        }
    }
}

/// Notification carrying one event to the client.
#[derive(Serialize)]
struct EventNotification<'a> {
    jsonrpc: &'static str,
    method: &'static str,
    params: &'a Event,
}

/// Serve a connection whose first request, `attach`, has been read.
///
/// `buffered` holds what the client sent after the `attach` line.
pub(super) async fn serve<S>(
    stream: S,
    attach: JsonRpcRequest,
    buffered: Vec<u8>,
    ctx: &ConnectionContext,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let cancel = ctx.cancel.child_token();

    let (events, response) = match attach_connection(ctx, attach) {
        Ok(attached) => attached,
        Err(response) => return write_line(&mut writer, &*response, &cancel).await,
    };
    let connection = events.id();
    // Everything the client asked to subscribe to is registered by now, so no
    // event published from here on is missed; the events queue up behind
    // this response.
    write_line(&mut writer, &response, &cancel).await?;
    log::debug!("IPC connection {connection} attached");

    let (responses_tx, responses_rx) = mpsc::channel(RESPONSE_QUEUE_CAPACITY);
    let (read_result, write_result) = tokio::join!(
        read_requests(reader, buffered, responses_tx, connection, ctx, &cancel),
        write_outgoing(writer, responses_rx, events, ctx, &cancel),
    );
    log::debug!("IPC connection {connection} closed");
    read_result.and(write_result)
}

/// Validate `attach`, register the connection with the bus and build the
/// response. `Err` carries the error response; the connection is then closed.
fn attach_connection(
    ctx: &ConnectionContext,
    request: JsonRpcRequest,
) -> Result<(EventStream, JsonRpcResponse), Box<JsonRpcResponse>> {
    let id = request.id;
    let params = if request.params.is_null() {
        serde_json::Value::Object(Default::default())
    } else {
        request.params
    };
    let params: AttachParams = parse_params(&id, params)?;

    let protocol = params.protocol.unwrap_or(PROTOCOL_VERSION);
    if protocol != PROTOCOL_VERSION {
        return Err(Box::new(JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            format!(
                "Unsupported protocol version {protocol}; this server speaks {PROTOCOL_VERSION}"
            ),
        )));
    }
    let patterns = parse_patterns(&params.subscribe)
        .map_err(|message| Box::new(JsonRpcResponse::error(id.clone(), INVALID_PARAMS, message)))?;

    let events = ctx.bus.register_connection();
    ctx.bus.subscribe_many(events.id(), patterns).map_err(|e| {
        Box::new(JsonRpcResponse::error(
            id.clone(),
            INVALID_PARAMS,
            e.to_string(),
        ))
    })?;

    if let Some(client) = &params.client {
        log::debug!(
            "IPC connection {} attached by client {:?} {:?}",
            events.id(),
            client.name,
            client.version
        );
    }

    let session = serde_json::to_value(collect_session_result(false, ATTACH_SESSION_REASON))
        .map_err(|e| {
            Box::new(JsonRpcResponse::error(
                id.clone(),
                INTERNAL_ERROR,
                format!("Session info error: {e}"),
            ))
        })?;
    let result = serde_json::json!({
        "connection_id": events.id(),
        "api_version": API_VERSION,
        "capabilities": ["events"],
        "session_id": session_id(),
        "session": session,
    });
    Ok((events, JsonRpcResponse::success(id, result)))
}

/// Parse every pattern, so that either all of them take effect or none does.
fn parse_patterns(patterns: &[String]) -> Result<Vec<SubscriptionPattern>, String> {
    patterns
        .iter()
        .map(|pattern| {
            SubscriptionPattern::parse(pattern)
                .map_err(|e| format!("Invalid subscription pattern {pattern:?}: {e}"))
        })
        .collect()
}

/// Read lines from the client until the connection ends.
///
/// `buffered` is input that was already read from the stream. Ends on EOF, a
/// read error, a protocol violation, or when `cancel` fires. Dropping
/// `responses` on the way out tells the writer that nothing more will come.
async fn read_requests<R>(
    mut reader: R,
    buffered: Vec<u8>,
    responses: mpsc::Sender<Outgoing>,
    connection: ConnectionId,
    ctx: &ConnectionContext,
    cancel: &CancellationToken,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut lines = RequestLoop {
        line: Vec::new(),
        requests: JoinSet::new(),
        responses,
        connection,
        ctx,
        cancel,
    };
    if !lines.feed(&buffered).await {
        return Ok(());
    }

    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            joined = lines.requests.join_next(), if !lines.requests.is_empty() => {
                if let Some(joined) = joined
                    && !lines.finish(joined).await
                {
                    return Ok(());
                }
            }
            read = reader.read(&mut chunk) => {
                let n = read?;
                if n == 0 || !lines.feed(&chunk[..n]).await {
                    return Ok(());
                }
            }
        }
    }
}

/// State of the reading half of a long-lived connection.
struct RequestLoop<'a> {
    /// The start of the line being received, if it is not complete yet.
    line: Vec<u8>,
    /// Requests running on behalf of the client. Dropping the set drops them.
    requests: JoinSet<Outgoing>,
    responses: mpsc::Sender<Outgoing>,
    connection: ConnectionId,
    ctx: &'a ConnectionContext,
    cancel: &'a CancellationToken,
}

impl RequestLoop<'_> {
    /// Process newly received bytes. Returns `false` if the connection must
    /// be closed.
    ///
    /// Only the new bytes are searched for a newline, so reading a long line
    /// in pieces takes linear time.
    async fn feed(&mut self, mut data: &[u8]) -> bool {
        while let Some(end) = data.iter().position(|byte| *byte == b'\n') {
            let (head, tail) = (&data[..end], &data[end + 1..]);
            data = tail;
            if self.line.len() + head.len() > MAX_REQUEST_BYTES {
                return self.line_too_long();
            }
            let keep_open = if self.line.is_empty() {
                self.handle_line(head).await
            } else {
                self.line.extend_from_slice(head);
                let line = std::mem::take(&mut self.line);
                self.handle_line(&line).await
            };
            if !keep_open {
                return false;
            }
        }
        if self.line.len() + data.len() > MAX_REQUEST_BYTES {
            return self.line_too_long();
        }
        self.line.extend_from_slice(data);
        true
    }

    fn line_too_long(&self) -> bool {
        log::debug!(
            "IPC connection {} sent a line over {MAX_REQUEST_BYTES} bytes",
            self.connection
        );
        false
    }

    /// Handle one complete line. Returns `false` if the connection must be
    /// closed.
    async fn handle_line(&mut self, line: &[u8]) -> bool {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.iter().all(u8::is_ascii_whitespace) {
            return true;
        }
        let value: serde_json::Value = match serde_json::from_slice(line) {
            Ok(value) => value,
            Err(e) => {
                log::debug!("IPC connection {} sent invalid JSON: {e}", self.connection);
                return false;
            }
        };

        let id = value.get("id").filter(|id| !id.is_null()).cloned();
        let request: JsonRpcRequest = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(e) => {
                let response =
                    JsonRpcResponse::error(id, INVALID_REQUEST, format!("Invalid request: {e}"));
                return self.respond(response).await;
            }
        };
        if request.jsonrpc != "2.0" {
            let response = JsonRpcResponse::error(
                request.id,
                INVALID_REQUEST,
                "Invalid JSON-RPC version".to_string(),
            );
            return self.respond(response).await;
        }
        // Notifications get no response and, as for single-shot requests,
        // are not carried out.
        if request.id.is_none() {
            return true;
        }

        match request.method.as_str() {
            ATTACH_METHOD => {
                let response = JsonRpcResponse::error(
                    request.id,
                    INVALID_REQUEST,
                    "This connection is already attached".to_string(),
                );
                self.respond(response).await
            }
            "subscribe" => {
                let response = self.subscribe(request.id, request.params);
                self.respond(response).await
            }
            "unsubscribe" => {
                let response = self.unsubscribe(request.id, request.params);
                self.respond(response).await
            }
            _ => self.start_request(request).await,
        }
    }

    fn subscribe(
        &self,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> JsonRpcResponse {
        let patterns = match self.patterns(&id, params) {
            Ok(patterns) => patterns,
            Err(response) => return *response,
        };
        match self.ctx.bus.subscribe_many(self.connection, patterns) {
            Ok(added) => JsonRpcResponse::success(id, serde_json::json!({ "added": added })),
            Err(e @ SubscribeError::TooManySubscriptions { .. }) => {
                JsonRpcResponse::error(id, INVALID_PARAMS, e.to_string())
            }
            Err(e) => JsonRpcResponse::error(id, INTERNAL_ERROR, e.to_string()),
        }
    }

    fn unsubscribe(
        &self,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> JsonRpcResponse {
        let patterns = match self.patterns(&id, params) {
            Ok(patterns) => patterns,
            Err(response) => return *response,
        };
        let removed = patterns
            .iter()
            .filter(|pattern| self.ctx.bus.unsubscribe(self.connection, pattern))
            .count();
        JsonRpcResponse::success(id, serde_json::json!({ "removed": removed }))
    }

    /// Parse the `patterns` parameter of `subscribe` / `unsubscribe`.
    fn patterns(
        &self,
        id: &Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> Result<Vec<SubscriptionPattern>, Box<JsonRpcResponse>> {
        let params: PatternsParams = parse_params(id, params)?;
        parse_patterns(&params.patterns).map_err(|message| {
            Box::new(JsonRpcResponse::error(id.clone(), INVALID_PARAMS, message))
        })
    }

    /// Run a request in the background, or refuse it when too many are
    /// outstanding. Returns `false` if the connection must be closed.
    async fn start_request(&mut self, request: JsonRpcRequest) -> bool {
        if self.requests.len() >= MAX_OUTSTANDING_REQUESTS {
            let response = JsonRpcResponse::error(
                request.id,
                R_BUSY,
                format!(
                    "Too many outstanding requests on this connection (at most \
                     {MAX_OUTSTANDING_REQUESTS})"
                ),
            );
            return self.respond(response).await;
        }
        let tx = self.ctx.tx.clone();
        self.requests.spawn(async move {
            let is_shutdown = request.method == "shutdown";
            let response = dispatch_request(request, &tx).await;
            Outgoing {
                shutdown_accepted: is_shutdown && is_accepted_shutdown(&response),
                response,
            }
        });
        true
    }

    /// Hand the result of a finished request to the writer.
    async fn finish(&self, joined: Result<Outgoing, JoinError>) -> bool {
        match joined {
            Ok(outgoing) => self.send(outgoing).await,
            Err(e) => {
                log::error!("IPC request on connection {} failed: {e}", self.connection);
                false
            }
        }
    }

    async fn respond(&self, response: JsonRpcResponse) -> bool {
        self.send(Outgoing::respond(response)).await
    }

    /// Queue `outgoing` for the writer, waiting for room. Returns `false` if
    /// the writer is gone or the connection is being cancelled.
    async fn send(&self, outgoing: Outgoing) -> bool {
        tokio::select! {
            sent = self.responses.send(outgoing) => sent.is_ok(),
            () = self.cancel.cancelled() => false,
        }
    }
}

/// Write responses and events to the client until the connection ends.
///
/// Ends when the reader has finished and its queued responses are written,
/// the bus disconnects the connection, a write fails or times out, or
/// `cancel` fires. Cancels `cancel` on the way out so the reader stops too.
async fn write_outgoing<W>(
    mut writer: W,
    mut responses: mpsc::Receiver<Outgoing>,
    mut events: EventStream,
    ctx: &ConnectionContext,
    cancel: &CancellationToken,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let _close_connection = cancel.clone().drop_guard();
    loop {
        // Both sources are always polled in random order, so a steady stream
        // of one cannot keep the other waiting.
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            outgoing = responses.recv() => {
                let Some(outgoing) = outgoing else { return Ok(()) };
                let written = write_line(&mut writer, &outgoing.response, cancel).await;
                // Whether or not the client got the answer, it must not be
                // able to hold the shutdown back by not reading it.
                if outgoing.shutdown_accepted {
                    (ctx.request_shutdown)();
                }
                written?;
            }
            event = events.recv() => {
                let Some(event) = event else {
                    log::debug!(
                        "IPC connection {} lost its event stream: {:?}",
                        events.id(),
                        events.disconnect_reason()
                    );
                    return Ok(());
                };
                let notification = EventNotification {
                    jsonrpc: "2.0",
                    method: "event",
                    params: &event,
                };
                write_line(&mut writer, &notification, cancel).await?;
            }
        }
    }
}

/// Write `value` as one line, within [`WRITE_TIMEOUT`] and unless `cancel`
/// fires first. After an error the line may be partly written, so the
/// connection must be closed.
async fn write_line<W>(
    writer: &mut W,
    value: &impl Serialize,
    cancel: &CancellationToken,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    let write = async {
        writer.write_all(&line).await?;
        writer.flush().await
    };
    tokio::select! {
        result = tokio::time::timeout(WRITE_TIMEOUT, write) => match result {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out writing to the client",
            )),
        },
        () = cancel.cancelled() => Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "connection closed while writing",
        )),
    }
}

#[cfg(test)]
mod tests;
