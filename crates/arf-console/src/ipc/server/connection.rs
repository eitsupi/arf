//! Per-connection request handling: HTTP framing, raw JSON-RPC, and response
//! writing.
//!
//! A connection is single-shot (one request, one response, then close) unless
//! its first request is an `attach` sent as newline-terminated raw JSON, which
//! turns it into a long-lived connection (see [`super::attached`]).

use super::attached;
use super::dispatch::dispatch_request;
use crate::ipc::events::EventBus;
use crate::ipc::protocol::{
    INVALID_REQUEST, IpcRequest, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR,
};
use crate::ipc::state::trigger_headless_shutdown;
use std::sync::{Arc, mpsc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// Maximum accepted request size (1MB).
pub(super) const MAX_REQUEST_BYTES: usize = 1_048_576;

/// Size of each read from the stream. The reader never buffers more than
/// [`MAX_REQUEST_BYTES`] plus one chunk.
pub(super) const READ_CHUNK_BYTES: usize = 4096;

/// Method keywords (with the trailing space) that mark a request as HTTP.
const HTTP_METHOD_PREFIXES: [&[u8]; 3] = [b"POST ", b"GET ", b"PUT "];

/// Method that turns a connection into a long-lived one.
pub(super) const ATTACH_METHOD: &str = "attach";

/// What a connection handler needs from the server that accepted it.
#[derive(Clone)]
pub(super) struct ConnectionContext {
    /// Channel to the main thread.
    pub(super) tx: mpsc::Sender<IpcRequest>,
    /// Bus that long-lived connections receive events from.
    pub(super) bus: EventBus,
    /// Cancelled when the server stops.
    pub(super) cancel: CancellationToken,
    /// Called once an accepted `shutdown` has been answered (or the answer
    /// could not be delivered).
    pub(super) request_shutdown: Arc<dyn Fn() + Send + Sync>,
}

impl ConnectionContext {
    pub(super) fn new(
        tx: mpsc::Sender<IpcRequest>,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            tx,
            bus,
            cancel,
            request_shutdown: Arc::new(|| {
                trigger_headless_shutdown();
            }),
        }
    }
}

/// The first request frame read from a connection.
struct FirstFrame {
    /// The JSON body of the frame. For HTTP the headers are already stripped.
    /// Empty if the peer closed without sending anything, or if an HTTP
    /// request carried no body. If the stream ended or the data was malformed
    /// before a complete value, this holds whatever was received so that
    /// parsing it reports the error.
    body: Vec<u8>,
    /// How the frame was framed. A connection that sent nothing at all is
    /// [`Framing::RawJson`].
    framing: Framing,
    /// Bytes that arrived after the frame (for example the start of the next
    /// newline-delimited frame). Preserved exactly as received.
    remainder: Vec<u8>,
}

/// Outcome of reading the first frame from a stream.
enum RequestRead {
    Complete(FirstFrame),
    /// The frame exceeded [`MAX_REQUEST_BYTES`].
    TooLarge,
}

/// Progress of framing the bytes buffered so far.
enum FrameScan {
    /// The frame is the buffered bytes `body_start..body_end`; everything
    /// after `body_end` belongs to whatever follows the frame.
    Complete {
        body_start: usize,
        body_end: usize,
    },
    /// More data is needed. If the stream ends now, the body is the buffered
    /// bytes from `body_start` on.
    Incomplete {
        body_start: usize,
    },
    TooLarge,
}

/// How the first bytes of a connection are framed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Framing {
    Http,
    RawJson,
    /// Too few bytes to tell whether a method keyword is being sent.
    Undecided,
}

/// Handle a connection: read the first request, then either dispatch it and
/// respond (single-shot) or hand the connection to [`attached::serve`].
pub(super) async fn handle_connection<S>(
    mut stream: S,
    ctx: ConnectionContext,
) -> std::io::Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let frame = match read_first_frame(&mut stream).await? {
        RequestRead::Complete(frame) => frame,
        RequestRead::TooLarge => {
            let response =
                JsonRpcResponse::error(None, INVALID_REQUEST, "Request too large".to_string());
            return write_response(&mut stream, &response).await;
        }
    };

    // Only a connection that sent no bytes closes silently. An HTTP request
    // with an empty body still gets a parse error response.
    if frame.body.is_empty() && frame.framing == Framing::RawJson {
        return Ok(());
    }

    let request = match parse_request(&frame.body) {
        Ok(request) => request,
        Err(response) => return write_response(&mut stream, &response).await,
    };

    if request.method == ATTACH_METHOD {
        // A notification gets no response, whatever its framing.
        if request.id.is_none() {
            return Ok(());
        }
        if frame.framing == Framing::Http {
            let response = JsonRpcResponse::error(
                request.id,
                INVALID_REQUEST,
                "attach requires a raw newline-delimited connection".to_string(),
            );
            return write_response(&mut stream, &response).await;
        }
        if let Some(buffered) =
            read_until_line_end(&mut stream, frame.body.len(), frame.remainder).await?
        {
            return attached::serve(stream, request, buffered, &ctx).await;
        }
        // Not newline-terminated: an ordinary single-shot request.
    }

    // JSON-RPC 2.0 notifications (absent id) must not yield a response.
    if request.id.is_none() {
        return Ok(());
    }

    // Dispatch based on method
    let is_shutdown = request.method == "shutdown";
    let response = dispatch_request(request, &ctx.tx).await;

    // Send HTTP response (for curl compatibility)
    let accepted_shutdown = is_shutdown && is_accepted_shutdown(&response);
    let result = write_response(&mut stream, &response).await;
    if accepted_shutdown {
        (ctx.request_shutdown)();
    }
    result
}

/// Whether `response` is the result of a `shutdown` request that was accepted.
pub(super) fn is_accepted_shutdown(response: &JsonRpcResponse) -> bool {
    response
        .result
        .as_ref()
        .and_then(|result| result.get("accepted"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

/// Check that the first request frame ends its line, reading more if needed.
///
/// `line_len` is the length of the frame, and `pending` the bytes that were
/// read after it. Only spaces, tabs and `\r` may precede the newline. Returns
/// the bytes following the newline, or `None` if the line holds anything else,
/// is longer than [`MAX_REQUEST_BYTES`], or the stream ended first.
async fn read_until_line_end<S>(
    stream: &mut S,
    mut line_len: usize,
    mut pending: Vec<u8>,
) -> std::io::Result<Option<Vec<u8>>>
where
    S: AsyncReadExt + Unpin,
{
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        let blanks = pending
            .iter()
            .take_while(|byte| matches!(**byte, b' ' | b'\t' | b'\r'))
            .count();
        line_len += blanks;
        pending.drain(..blanks);
        match pending.first() {
            Some(b'\n') => return Ok(Some(pending.split_off(1))),
            Some(_) => return Ok(None),
            None => {}
        }
        if line_len > MAX_REQUEST_BYTES {
            return Ok(None);
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        pending.extend_from_slice(&chunk[..n]);
    }
}

/// Read the first request frame from the stream (up to 1MB).
///
/// The framing is decided by the first bytes:
/// 1. HTTP (starts with a method keyword, for curl compatibility): read the
///    headers, then exactly `Content-Length` body bytes, or until EOF if the
///    header is absent.
/// 2. Raw JSON: read until the first complete JSON value. Bytes after it are
///    returned in [`FirstFrame::remainder`] instead of being treated as part
///    of the request.
///
/// The size limit is enforced while reading, so an oversized request is
/// rejected without buffering more than the limit plus one read chunk.
async fn read_first_frame<S>(stream: &mut S) -> std::io::Result<RequestRead>
where
    S: AsyncReadExt + Unpin,
{
    let mut buf = Vec::with_capacity(READ_CHUNK_BYTES);
    let mut chunk = [0u8; READ_CHUNK_BYTES];

    loop {
        let n = stream.read(&mut chunk).await?;
        buf.extend_from_slice(&chunk[..n]);

        let (body_start, body_end) = match scan_first_frame(&buf) {
            FrameScan::Complete {
                body_start,
                body_end,
            } => (body_start, body_end),
            FrameScan::TooLarge => return Ok(RequestRead::TooLarge),
            FrameScan::Incomplete { body_start } if n == 0 => (body_start, buf.len()),
            FrameScan::Incomplete { .. } => continue,
        };

        // A stream that ended before a method keyword was complete (including
        // one that sent nothing) is treated as raw JSON.
        let framing = match classify_framing(&buf) {
            Framing::Http => Framing::Http,
            Framing::RawJson | Framing::Undecided => Framing::RawJson,
        };
        let remainder = buf.split_off(body_end);
        buf.drain(..body_start);
        return Ok(RequestRead::Complete(FirstFrame {
            body: buf,
            framing,
            remainder,
        }));
    }
}

/// Determine how much of `buf` forms the first frame.
fn scan_first_frame(buf: &[u8]) -> FrameScan {
    match classify_framing(buf) {
        Framing::Http => scan_http(buf),
        Framing::RawJson => scan_raw_json(buf),
        Framing::Undecided => need_more(buf.len(), 0),
    }
}

fn classify_framing(buf: &[u8]) -> Framing {
    if HTTP_METHOD_PREFIXES
        .iter()
        .any(|method| buf.starts_with(method))
    {
        Framing::Http
    } else if HTTP_METHOD_PREFIXES
        .iter()
        .any(|method| method.starts_with(buf))
    {
        // Includes the empty buffer.
        Framing::Undecided
    } else {
        Framing::RawJson
    }
}

/// Ask for more data, unless the buffered bytes already exceed the limit.
fn need_more(buffered: usize, body_start: usize) -> FrameScan {
    if buffered > MAX_REQUEST_BYTES {
        FrameScan::TooLarge
    } else {
        FrameScan::Incomplete { body_start }
    }
}

/// Frame an HTTP request: headers, then the body.
fn scan_http(buf: &[u8]) -> FrameScan {
    let Some(header_end) = find_http_header_end(buf) else {
        return need_more(buf.len(), 0);
    };
    let body_start = header_end + 4;

    match parse_content_length(&buf[..header_end]) {
        Some(body_len) => {
            // Use checked_add to prevent overflow bypassing the size limit
            match body_start.checked_add(body_len) {
                Some(body_end) if body_end <= MAX_REQUEST_BYTES => {
                    if buf.len() >= body_end {
                        FrameScan::Complete {
                            body_start,
                            body_end,
                        }
                    } else {
                        FrameScan::Incomplete { body_start }
                    }
                }
                _ => FrameScan::TooLarge,
            }
        }
        // Without Content-Length the body runs until the peer closes.
        None => need_more(buf.len(), body_start),
    }
}

/// Frame raw JSON: the first complete JSON value, preceded by optional
/// whitespace.
fn scan_raw_json(buf: &[u8]) -> FrameScan {
    let mut values = serde_json::Deserializer::from_slice(buf).into_iter::<serde::de::IgnoredAny>();
    match values.next() {
        Some(Ok(_)) => {
            let body_end = values.byte_offset();
            if body_end > MAX_REQUEST_BYTES {
                return FrameScan::TooLarge;
            }
            FrameScan::Complete {
                body_start: 0,
                body_end,
            }
        }
        // Whitespace only, or a value cut off by the end of the buffer.
        None => need_more(buf.len(), 0),
        Some(Err(e)) if e.is_eof() => need_more(buf.len(), 0),
        // Malformed: hand everything over so that parsing reports the error.
        Some(Err(_)) => FrameScan::Complete {
            body_start: 0,
            body_end: buf.len(),
        },
    }
}

/// Parse and validate a JSON-RPC request body.
///
/// `Err` carries the error response to send back.
fn parse_request(body: &[u8]) -> Result<JsonRpcRequest, Box<JsonRpcResponse>> {
    let request: JsonRpcRequest = serde_json::from_slice(body).map_err(|e| {
        Box::new(JsonRpcResponse::error(
            None,
            PARSE_ERROR,
            format!("Parse error: {e}"),
        ))
    })?;

    // Validate jsonrpc version
    if request.jsonrpc != "2.0" {
        return Err(Box::new(JsonRpcResponse::error(
            request.id,
            INVALID_REQUEST,
            "Invalid JSON-RPC version".to_string(),
        )));
    }

    Ok(request)
}

/// Write a JSON-RPC response as an HTTP response (for curl compatibility).
async fn write_response<S>(stream: &mut S, response: &JsonRpcResponse) -> std::io::Result<()>
where
    S: AsyncWriteExt + Unpin,
{
    let json = serde_json::to_string(response).unwrap_or_default();
    let http_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        json.len(),
        json
    );
    stream.write_all(http_response.as_bytes()).await
}

/// Find the position of the end of HTTP headers (`\r\n\r\n`).
/// Returns the byte offset of the first `\r` in the blank line, or None.
fn find_http_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parse the Content-Length header value from HTTP headers (case-insensitive).
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let header_str = std::str::from_utf8(headers).ok()?;
    let prefix = "content-length:";
    for line in header_str.split("\r\n") {
        if line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return line[prefix.len()..].trim().parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::events::Limits;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{AsyncRead, ReadBuf};

    /// Reader that yields the given chunks one per read, then EOF.
    struct ChunkedReader {
        chunks: VecDeque<Vec<u8>>,
    }

    impl ChunkedReader {
        fn new<I>(chunks: I) -> Self
        where
            I: IntoIterator,
            I::Item: AsRef<[u8]>,
        {
            Self {
                chunks: chunks.into_iter().map(|c| c.as_ref().to_vec()).collect(),
            }
        }

        /// Split `data` into chunks of `size` bytes.
        fn split(data: &[u8], size: usize) -> Self {
            Self::new(data.chunks(size))
        }
    }

    impl AsyncRead for ChunkedReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(mut chunk) = self.chunks.pop_front() {
                let n = chunk.len().min(buf.remaining());
                buf.put_slice(&chunk[..n]);
                if n < chunk.len() {
                    self.chunks.push_front(chunk.split_off(n));
                }
            }
            Poll::Ready(Ok(()))
        }
    }

    /// Reader that never reaches EOF and counts the bytes it has handed out.
    struct EndlessReader {
        byte: u8,
        served: usize,
    }

    impl AsyncRead for EndlessReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let n = buf.remaining();
            let byte = self.byte;
            buf.put_slice(&vec![byte; n]);
            self.served += n;
            Poll::Ready(Ok(()))
        }
    }

    async fn read_frame<S: AsyncReadExt + Unpin>(mut stream: S) -> (Vec<u8>, Vec<u8>) {
        match read_first_frame(&mut stream).await.unwrap() {
            RequestRead::Complete(frame) => (frame.body, frame.remainder),
            RequestRead::TooLarge => panic!("unexpected TooLarge"),
        }
    }

    async fn read_too_large<S: AsyncReadExt + Unpin>(mut stream: S) {
        assert!(matches!(
            read_first_frame(&mut stream).await.unwrap(),
            RequestRead::TooLarge
        ));
    }

    const REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"status","params":{"a":[1,2,"x}"]}}"#;

    #[tokio::test]
    async fn test_raw_json_in_one_read() {
        let (body, rest) = read_frame(ChunkedReader::new([REQUEST])).await;
        assert_eq!(body, REQUEST.as_bytes());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_raw_json_split_across_many_small_reads() {
        for size in [1, 2, 3, 7] {
            let (body, rest) = read_frame(ChunkedReader::split(REQUEST.as_bytes(), size)).await;
            assert_eq!(body, REQUEST.as_bytes(), "chunk size {size}");
            assert!(rest.is_empty(), "chunk size {size}");
        }
    }

    #[tokio::test]
    async fn test_raw_json_larger_than_one_read_chunk() {
        let padding = "x".repeat(READ_CHUNK_BYTES * 3 + 17);
        let request = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"m","params":"{padding}"}}"#);
        let (body, rest) = read_frame(ChunkedReader::new([request.as_str()])).await;
        assert_eq!(body, request.as_bytes());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_raw_json_pretty_printed() {
        let pretty = r#"
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "status",
  "params": {
    "list": [
      1,
      2
    ]
  }
}"#;
        let (body, rest) = read_frame(ChunkedReader::split(pretty.as_bytes(), 5)).await;
        assert_eq!(body, pretty.as_bytes());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_raw_json_with_trailing_newline() {
        let data = format!("{REQUEST}\n");
        let (body, rest) = read_frame(ChunkedReader::new([data.as_str()])).await;
        assert_eq!(body, REQUEST.as_bytes());
        assert_eq!(rest, b"\n");
    }

    #[tokio::test]
    async fn test_two_values_in_one_read_preserves_remainder() {
        let second = r#"{"jsonrpc":"2.0","id":2,"method":"next"}"#;
        let data = format!("{REQUEST}\n{second}\n");
        let (body, rest) = read_frame(ChunkedReader::new([data.as_str()])).await;
        assert_eq!(body, REQUEST.as_bytes());
        assert_eq!(rest, format!("\n{second}\n").as_bytes());
    }

    #[tokio::test]
    async fn test_remainder_may_hold_a_partial_value() {
        let data = format!(r#"{REQUEST}{}"#, r#"{"jsonrpc":"#);
        let (body, rest) = read_frame(ChunkedReader::new([data.as_str()])).await;
        assert_eq!(body, REQUEST.as_bytes());
        assert_eq!(rest, br#"{"jsonrpc":"#);
    }

    #[tokio::test]
    async fn test_bare_number_completes_without_waiting_for_eof() {
        // JSON-RPC requests are objects, so a number frame is invalid however
        // many digits follow. The client keeps the connection open (the writer
        // is alive), so the reader must answer without waiting for more data.
        let (mut client, mut server) = tokio::io::duplex(64);
        client.write_all(b"1").await.unwrap();

        let frame = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut server))
            .await
            .expect("a bare number must complete the frame without EOF");
        assert_eq!(frame.0, b"1");
        assert!(frame.1.is_empty());
        assert!(parse_request(&frame.0).is_err());
        drop(client);
    }

    #[tokio::test]
    async fn test_empty_connection() {
        let (body, rest) = read_frame(ChunkedReader::new(Vec::<&str>::new())).await;
        assert!(body.is_empty());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_eof_before_complete_raw_value_returns_partial_bytes() {
        let partial = &REQUEST[..REQUEST.len() - 5];
        let (body, rest) = read_frame(ChunkedReader::split(partial.as_bytes(), 9)).await;
        assert_eq!(body, partial.as_bytes());
        assert!(rest.is_empty());
        assert!(parse_request(&body).is_err());
    }

    #[tokio::test]
    async fn test_eof_with_whitespace_only() {
        let (body, rest) = read_frame(ChunkedReader::new(["  \n"])).await;
        assert_eq!(body, b"  \n");
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_malformed_json_is_returned_without_waiting_for_eof() {
        // The stream stays open after the garbage; the reader must not wait.
        let mut reader = EndlessReader {
            byte: b'}',
            served: 0,
        };
        let (body, _) = read_frame(&mut reader).await;
        assert_eq!(reader.served, READ_CHUNK_BYTES);
        assert_eq!(body.len(), READ_CHUNK_BYTES);
        assert!(parse_request(&body).is_err());
    }

    #[tokio::test]
    async fn test_http_post_in_one_read() {
        let http = format!(
            "POST / HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{REQUEST}",
            REQUEST.len()
        );
        let (body, rest) = read_frame(ChunkedReader::new([http.as_str()])).await;
        assert_eq!(body, REQUEST.as_bytes());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_http_content_length_split_across_reads() {
        let http = format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\ncontent-length: {}\r\n\r\n{REQUEST}",
            REQUEST.len()
        );
        for size in [1, 4, 11] {
            let (body, rest) = read_frame(ChunkedReader::split(http.as_bytes(), size)).await;
            assert_eq!(body, REQUEST.as_bytes(), "chunk size {size}");
            assert!(rest.is_empty(), "chunk size {size}");
        }
    }

    #[tokio::test]
    async fn test_http_bytes_beyond_content_length_are_the_remainder() {
        let http = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n{REQUEST}EXTRA",
            REQUEST.len()
        );
        let (body, rest) = read_frame(ChunkedReader::new([http.as_str()])).await;
        assert_eq!(body, REQUEST.as_bytes());
        assert_eq!(rest, b"EXTRA");
    }

    #[tokio::test]
    async fn test_http_method_keyword_split_across_reads() {
        let rest_of_head = format!(" / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", REQUEST.len());
        let (body, rest) = read_frame(ChunkedReader::new([
            "G",
            "ET",
            rest_of_head.as_str(),
            REQUEST,
        ]))
        .await;
        assert_eq!(body, REQUEST.as_bytes());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_http_without_content_length_reads_until_eof() {
        let http = format!("POST / HTTP/1.1\r\nHost: localhost\r\n\r\n{REQUEST}");
        let (body, rest) = read_frame(ChunkedReader::split(http.as_bytes(), 16)).await;
        assert_eq!(body, REQUEST.as_bytes());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_http_body_shorter_than_content_length_returns_what_arrived() {
        let http = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            REQUEST.len(),
            &REQUEST[..10]
        );
        let (body, rest) = read_frame(ChunkedReader::new([http.as_str()])).await;
        assert_eq!(body, &REQUEST.as_bytes()[..10]);
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn test_http_content_length_over_limit_is_rejected_before_the_body() {
        let http = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_REQUEST_BYTES + 1
        );
        read_too_large(ChunkedReader::new([http.as_str()])).await;
    }

    #[tokio::test]
    async fn test_http_content_length_overflow_is_rejected() {
        let http = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", usize::MAX);
        read_too_large(ChunkedReader::new([http.as_str()])).await;
    }

    #[tokio::test]
    async fn test_http_endless_headers_hit_the_limit_during_reading() {
        let mut reader = EndlessReader {
            byte: b'a',
            served: 0,
        };
        let mut stream = ChunkedReader::new(["POST / HTTP/1.1\r\nX: "]).chain(&mut reader);
        read_too_large(&mut stream).await;
        assert!(reader.served <= MAX_REQUEST_BYTES + READ_CHUNK_BYTES);
    }

    #[tokio::test]
    async fn test_http_endless_body_without_content_length_hits_the_limit() {
        let mut reader = EndlessReader {
            byte: b'a',
            served: 0,
        };
        let mut stream = ChunkedReader::new(["POST / HTTP/1.1\r\n\r\n"]).chain(&mut reader);
        read_too_large(&mut stream).await;
        assert!(reader.served <= MAX_REQUEST_BYTES + READ_CHUNK_BYTES);
    }

    #[tokio::test]
    async fn test_raw_json_limit_is_enforced_during_reading() {
        // A string that never closes: the stream never ends and the value is
        // never complete, so only the limit can stop the reader.
        let mut reader = EndlessReader {
            byte: b'a',
            served: 0,
        };
        let mut stream = ChunkedReader::new([r#"{"jsonrpc":"2.0","method":""#]).chain(&mut reader);
        read_too_large(&mut stream).await;
        assert!(reader.served <= MAX_REQUEST_BYTES + READ_CHUNK_BYTES);
    }

    #[tokio::test]
    async fn test_raw_json_value_ending_beyond_limit_is_too_large() {
        let padding = "a".repeat(MAX_REQUEST_BYTES);
        let request = format!(r#"{{"jsonrpc":"2.0","method":"{padding}"}}"#);
        read_too_large(ChunkedReader::new([request.as_str()])).await;
    }

    #[tokio::test]
    async fn test_raw_json_exactly_at_limit_is_accepted() {
        let head = r#"{"jsonrpc":"2.0","method":""#;
        let tail = r#""}"#;
        let padding = "a".repeat(MAX_REQUEST_BYTES - head.len() - tail.len());
        let request = format!("{head}{padding}{tail}");
        assert_eq!(request.len(), MAX_REQUEST_BYTES);
        let (body, rest) = read_frame(ChunkedReader::split(request.as_bytes(), 8192)).await;
        assert_eq!(body.len(), MAX_REQUEST_BYTES);
        assert!(rest.is_empty());
    }

    /// Run `handle_connection` over a client that sends `input` and then
    /// closes its write side. Returns everything the server wrote back.
    async fn respond_to(input: &[u8]) -> Vec<u8> {
        let (mut client, server) = tokio::io::duplex(8192);
        client.write_all(input).await.unwrap();
        client.shutdown().await.unwrap();
        // The request never reaches dispatch, so the receiver is not read.
        let (tx, _rx) = mpsc::channel();
        let (bus, _dispatcher) = EventBus::new(Limits::default());
        let ctx = ConnectionContext::new(tx, bus, CancellationToken::new());
        handle_connection(server, ctx).await.unwrap();
        let mut written = Vec::new();
        client.read_to_end(&mut written).await.unwrap();
        written
    }

    fn assert_parse_error_response(written: &[u8]) {
        let text = std::str::from_utf8(written).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
        let (_, json) = text.split_once("\r\n\r\n").unwrap();
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(value["error"]["code"], PARSE_ERROR, "{text}");
    }

    #[tokio::test]
    async fn test_http_with_zero_content_length_gets_a_parse_error() {
        let written = respond_to(b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n").await;
        assert_parse_error_response(&written);
    }

    #[tokio::test]
    async fn test_http_headers_then_eof_gets_a_parse_error() {
        let written = respond_to(b"POST / HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert_parse_error_response(&written);
    }

    #[tokio::test]
    async fn test_whitespace_only_connection_gets_a_parse_error() {
        assert_parse_error_response(&respond_to(b"  \n").await);
    }

    #[tokio::test]
    async fn test_empty_connection_writes_nothing() {
        assert!(respond_to(b"").await.is_empty());
    }

    #[test]
    fn test_find_http_header_end() {
        assert_eq!(
            find_http_header_end(b"POST / HTTP/1.1\r\n\r\nbody"),
            Some(15)
        );
        assert_eq!(find_http_header_end(b"POST / HTTP/1.1\r\n"), None);
    }
}
