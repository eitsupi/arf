//! Stdio bridge for a long-lived IPC connection.

use super::client;
use super::protocol::JsonRpcResponse;
use std::io::{self, BufReader, Read, Write};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::sync::{mpsc, watch};

const IO_CHUNK_BYTES: usize = 64 * 1024;
const QUEUE_CHUNKS: usize = 128;
const MAX_LINE_BYTES: usize = 1024 * 1024;

enum StdinState {
    Open,
    Eof,
    ReadError(String),
}

enum EnqueueResult {
    Complete,
    StdinExit(i32),
    StdoutError(String),
    SocketError(io::Error),
}

/// Attach to a running session and bridge its JSON lines to stdio.
pub(crate) fn cmd_attach(pid: Option<u32>, subscribe: &[String]) -> ! {
    if let Err(error) = ctrlc::set_handler(|| std::process::exit(0)) {
        client::exit_error(
            2,
            "SIGNAL_HANDLER_ERROR",
            &format!("Failed to install termination handler: {error}"),
            None,
            None,
        );
    }
    let session = client::resolve_session(pid);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            client::exit_error(
                2,
                "TRANSPORT_ERROR",
                &format!("Failed to create runtime: {error}"),
                None,
                None,
            )
        });
    let code = runtime.block_on(run_bridge(&session.socket_path, subscribe));
    std::process::exit(code)
}

async fn run_bridge(socket_path: &str, subscribe: &[String]) -> i32 {
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(QUEUE_CHUNKS);
    let (stdin_state_tx, mut stdin_state_rx) = watch::channel(StdinState::Open);
    std::thread::Builder::new()
        .name("arf-ipc-stdin".into())
        .spawn(move || stdin_worker(stdin_tx, stdin_state_tx))
        .unwrap_or_else(|error| {
            client::exit_error(
                2,
                "TRANSPORT_ERROR",
                &format!("Failed to start stdin reader: {error}"),
                None,
                None,
            )
        });

    let connect = connect(socket_path);
    tokio::pin!(connect);
    let stream = tokio::select! {
        result = &mut connect => match result {
            Ok(stream) => stream,
            Err(error) => return transport_error(error),
        },
        _ = wait_for_stdin_exit(&mut stdin_state_rx) => return stdin_exit_code(&stdin_state_rx),
    };
    let (reader, mut socket_writer) = tokio::io::split(stream);
    let mut reader = AsyncBufReader::new(reader);

    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(QUEUE_CHUNKS);
    let (stdout_error_tx, mut stdout_error_rx) = mpsc::channel::<String>(1);
    let (stdout_done_tx, stdout_done_rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("arf-ipc-stdout".into())
        .spawn(move || {
            stdout_worker(stdout_rx, stdout_error_tx);
            let _ = stdout_done_tx.send(());
        })
        .unwrap_or_else(|error| {
            client::exit_error(
                2,
                "TRANSPORT_ERROR",
                &format!("Failed to start stdout writer: {error}"),
                None,
                None,
            )
        });

    // Start the socket writer before waiting for the attach response. Any
    // stdin already read is forwarded after this attach line.
    let (socket_error_tx, mut socket_error_rx) = mpsc::channel::<io::Error>(1);
    let attach = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "arf-ipc-attach",
        "method": "attach",
        "params": { "subscribe": subscribe },
    });
    let mut attach_line = serde_json::to_vec(&attach).expect("serializing JSON value cannot fail");
    attach_line.push(b'\n');
    let mut socket_write_closed = false;
    tokio::select! {
        _ = wait_for_stdin_exit(&mut stdin_state_rx) => return stdin_exit_code(&stdin_state_rx),
        result = socket_writer.write_all(&attach_line) => if let Err(error) = result {
            return transport_error(error);
        },
    }
    tokio::spawn(async move {
        while let Some(bytes) = stdin_rx.recv().await {
            if let Err(error) = socket_writer.write_all(&bytes).await {
                let _ = socket_error_tx.send(error).await;
                while stdin_rx.recv().await.is_some() {}
                return;
            }
        }
    });

    // Keep the buffer across select! iterations. This preserves any bytes
    // read beyond the attach response and makes the read cancellation-safe.
    let mut initial = Vec::with_capacity(IO_CHUNK_BYTES);
    let attach_response = loop {
        match take_first_line(&mut initial, MAX_LINE_BYTES) {
            Ok(Some(line)) => break line,
            Ok(None) => {}
            Err(LineTooLong) => client::exit_error(
                2,
                "PROTOCOL_ERROR",
                "Attach response exceeded the line limit",
                None,
                None,
            ),
        }
        tokio::select! {
            result = reader.read_buf(&mut initial) => match result {
                Ok(0) => return transport_error(io::Error::new(io::ErrorKind::UnexpectedEof, "Server closed the connection before the attach response")),
                Ok(_) => {},
                Err(error) => return transport_error(error),
            },
            _ = wait_for_stdin_exit(&mut stdin_state_rx) => return stdin_exit_code(&stdin_state_rx),
            error = stdout_error_rx.recv() => if let Some(error) = error {
                client::exit_error(2, "OUTPUT_ERROR", &error, None, None);
            },
            // Keep reading after a closed write side: the server may have
            // refused attach and closed after writing its error response.
            error = socket_error_rx.recv(), if !socket_write_closed => match error {
                Some(error) if is_closed_socket_error(&error) => socket_write_closed = true,
                Some(error) => return transport_error(error),
                None => socket_write_closed = true,
            },
        }
    };
    let response: JsonRpcResponse = match serde_json::from_slice(&attach_response) {
        Ok(response) => response,
        Err(error) => client::exit_error(
            2,
            "PROTOCOL_ERROR",
            &format!("Invalid attach response: {error}"),
            None,
            None,
        ),
    };
    if let Some(error) = response.error {
        let (code, hint) = client::rpc_error_info(error.code);
        client::exit_error(4, code, &error.message, hint, error.data.as_ref());
    }
    if response.result.is_none() {
        client::exit_error(
            4,
            "EMPTY_RESPONSE",
            "Attach response has neither result nor error",
            None,
            None,
        );
    }
    match enqueue_chunks(
        &stdout_tx,
        &attach_response,
        &mut stdin_state_rx,
        &mut stdout_error_rx,
        &mut socket_error_rx,
        &mut socket_write_closed,
    )
    .await
    {
        EnqueueResult::Complete => {}
        EnqueueResult::StdinExit(code) => return code,
        EnqueueResult::StdoutError(error) => {
            client::exit_error(2, "OUTPUT_ERROR", &error, None, None)
        }
        EnqueueResult::SocketError(error) => return transport_error(error),
    }
    let read_ahead = std::mem::take(&mut initial);
    match enqueue_chunks(
        &stdout_tx,
        &read_ahead,
        &mut stdin_state_rx,
        &mut stdout_error_rx,
        &mut socket_error_rx,
        &mut socket_write_closed,
    )
    .await
    {
        EnqueueResult::Complete => {}
        EnqueueResult::StdinExit(code) => return code,
        EnqueueResult::StdoutError(error) => {
            client::exit_error(2, "OUTPUT_ERROR", &error, None, None)
        }
        EnqueueResult::SocketError(error) => return transport_error(error),
    }

    let mut chunk = vec![0; IO_CHUNK_BYTES];
    loop {
        tokio::select! {
            biased;
            _ = wait_for_stdin_exit(&mut stdin_state_rx) => return stdin_exit_code(&stdin_state_rx),
            error = stdout_error_rx.recv() => if let Some(error) = error {
                client::exit_error(2, "OUTPUT_ERROR", &error, None, None);
            },
            error = socket_error_rx.recv(), if !socket_write_closed => match error {
                Some(error) if is_closed_socket_error(&error) => socket_write_closed = true,
                Some(error) => return transport_error(error),
                None => socket_write_closed = true,
            },
            result = reader.read(&mut chunk) => match result {
                Ok(0) => return finish_stdout(stdout_tx, stdout_done_rx, &mut stdout_error_rx, &mut stdin_state_rx).await,
                Ok(length) => {
                    match enqueue_chunks(&stdout_tx, &chunk[..length], &mut stdin_state_rx, &mut stdout_error_rx, &mut socket_error_rx, &mut socket_write_closed).await {
                        EnqueueResult::Complete => {},
                        EnqueueResult::StdinExit(code) => return code,
                        EnqueueResult::StdoutError(error) => client::exit_error(2, "OUTPUT_ERROR", &error, None, None),
                        EnqueueResult::SocketError(error) => return transport_error(error),
                    }
                }
                Err(error) if is_closed_socket_error(&error) => return finish_stdout(stdout_tx, stdout_done_rx, &mut stdout_error_rx, &mut stdin_state_rx).await,
                Err(error) => return transport_error(error),
            },
        }
    }
}

struct LineTooLong;

/// Remove the first complete line, including its newline, from `buffer`.
/// Fails once the line, excluding its newline, is longer than `max_bytes`,
/// whether or not the newline has arrived yet.
fn take_first_line(buffer: &mut Vec<u8>, max_bytes: usize) -> Result<Option<Vec<u8>>, LineTooLong> {
    match buffer.iter().position(|byte| *byte == b'\n') {
        Some(end) if end > max_bytes => Err(LineTooLong),
        Some(end) => Ok(Some(buffer.drain(..=end).collect())),
        None if buffer.len() > max_bytes => Err(LineTooLong),
        None => Ok(None),
    }
}

async fn enqueue_chunks(
    sender: &mpsc::Sender<Vec<u8>>,
    bytes: &[u8],
    stdin_state: &mut watch::Receiver<StdinState>,
    stdout_errors: &mut mpsc::Receiver<String>,
    socket_errors: &mut mpsc::Receiver<io::Error>,
    socket_write_closed: &mut bool,
) -> EnqueueResult {
    for chunk in bytes.chunks(IO_CHUNK_BYTES) {
        let send = sender.send(chunk.to_vec());
        tokio::pin!(send);
        loop {
            tokio::select! {
                result = &mut send => match result {
                    Ok(()) => break,
                    Err(_) => return EnqueueResult::StdoutError("stdout worker stopped".into()),
                },
                _ = wait_for_stdin_exit(stdin_state) => return EnqueueResult::StdinExit(stdin_exit_code(stdin_state)),
                error = stdout_errors.recv() => if let Some(error) = error { return EnqueueResult::StdoutError(error); },
                error = socket_errors.recv(), if !*socket_write_closed => match error {
                    Some(error) if is_closed_socket_error(&error) => {
                        *socket_write_closed = true;
                    }
                    Some(error) => return EnqueueResult::SocketError(error),
                    None => *socket_write_closed = true,
                },
            }
        }
    }
    EnqueueResult::Complete
}

async fn finish_stdout(
    sender: mpsc::Sender<Vec<u8>>,
    mut done: tokio::sync::oneshot::Receiver<()>,
    errors: &mut mpsc::Receiver<String>,
    stdin_state: &mut watch::Receiver<StdinState>,
) -> i32 {
    drop(sender);
    tokio::select! {
        biased;
        error = errors.recv() => match error {
            Some(error) => client::exit_error(2, "OUTPUT_ERROR", &error, None, None),
            None => {
                tokio::select! {
                    _ = &mut done => 0,
                    _ = wait_for_stdin_exit(stdin_state) => stdin_exit_code(stdin_state),
                }
            },
        },
        _ = &mut done => 0,
        _ = wait_for_stdin_exit(stdin_state) => stdin_exit_code(stdin_state),
    }
}

fn is_closed_socket_error(error: &io::Error) -> bool {
    #[cfg(windows)]
    if error.raw_os_error() == Some(109) {
        return true;
    }
    error.kind() == io::ErrorKind::BrokenPipe
}

async fn wait_for_stdin_exit(receiver: &mut watch::Receiver<StdinState>) {
    loop {
        if !matches!(*receiver.borrow(), StdinState::Open) {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

fn stdin_exit_code(receiver: &watch::Receiver<StdinState>) -> i32 {
    match &*receiver.borrow() {
        StdinState::Eof => 0,
        StdinState::ReadError(error) => {
            client::exit_error(2, "STDIN_READ_ERROR", error, None, None)
        }
        StdinState::Open => 0,
    }
}

fn stdin_worker(sender: mpsc::Sender<Vec<u8>>, state: watch::Sender<StdinState>) {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut buffer = vec![0; IO_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                state.send_replace(StdinState::Eof);
                return;
            }
            Ok(length) => {
                if sender.blocking_send(buffer[..length].to_vec()).is_err() {
                    return;
                }
            }
            Err(error) => {
                state.send_replace(StdinState::ReadError(error.to_string()));
                return;
            }
        }
    }
}

fn stdout_worker(mut receiver: mpsc::Receiver<Vec<u8>>, errors: mpsc::Sender<String>) {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    while let Some(chunk) = receiver.blocking_recv() {
        let mut start = 0;
        for (index, byte) in chunk.iter().enumerate() {
            if *byte == b'\n' {
                if let Err(error) = stdout
                    .write_all(&chunk[start..=index])
                    .and_then(|_| stdout.flush())
                {
                    let _ = errors.blocking_send(format!("Failed to write stdout: {error}"));
                    return;
                }
                start = index + 1;
            }
        }
        if start < chunk.len()
            && let Err(error) = stdout.write_all(&chunk[start..])
        {
            let _ = errors.blocking_send(format!("Failed to write stdout: {error}"));
            return;
        }
    }
    if let Err(error) = stdout.flush() {
        let _ = errors.blocking_send(format!("Failed to flush stdout: {error}"));
    }
}

fn transport_error(error: io::Error) -> i32 {
    client::exit_error(
        2,
        "TRANSPORT_ERROR",
        &format!("IPC bridge transport failed: {error}"),
        Some("Check that the arf session is running and IPC is enabled."),
        None,
    )
}

async fn connect(path: &str) -> std::io::Result<Box<dyn AsyncConnection + Send + Unpin>> {
    #[cfg(unix)]
    {
        Ok(Box::new(tokio::net::UnixStream::connect(path).await?))
    }
    #[cfg(windows)]
    {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match tokio::net::windows::named_pipe::ClientOptions::new().open(path) {
                Ok(pipe) => return Ok(Box::new(pipe)),
                Err(error) if error.raw_os_error() == Some(231) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out waiting for the named pipe to become available",
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

trait AsyncConnection: tokio::io::AsyncRead + tokio::io::AsyncWrite {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite> AsyncConnection for T {}

#[cfg(test)]
mod tests {
    use super::{
        EnqueueResult, IO_CHUNK_BYTES, LineTooLong, StdinState, enqueue_chunks, finish_stdout,
        take_first_line,
    };
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    use tokio::sync::{mpsc, watch};

    #[test]
    fn attach_line_extraction_retains_partial_and_coalesced_read_ahead() {
        let mut buffered = br#"{"result":{}}"#.to_vec();
        assert!(matches!(take_first_line(&mut buffered, 64), Ok(None)));
        buffered.extend_from_slice(b"\nevent\nnext");
        assert_eq!(
            take_first_line(&mut buffered, 64).ok().flatten().unwrap(),
            concat!(r#"{"result":{}}"#, "\n").as_bytes()
        );
        assert_eq!(buffered, b"event\nnext");
    }

    #[test]
    fn attach_line_limit_applies_when_the_newline_arrives_with_the_excess() {
        let mut exact = b"1234\n".to_vec();
        assert!(matches!(take_first_line(&mut exact, 4), Ok(Some(_))));
        let mut oversized = b"12345\nevent\n".to_vec();
        assert!(matches!(
            take_first_line(&mut oversized, 4),
            Err(LineTooLong)
        ));
        let mut unterminated = b"12345".to_vec();
        assert!(matches!(
            take_first_line(&mut unterminated, 4),
            Err(LineTooLong)
        ));
    }

    #[tokio::test]
    async fn enqueue_waits_for_capacity_and_delivers_every_chunk() {
        let (sender, mut receiver) = mpsc::channel(1);
        let (_stdin_tx, mut stdin_state) = watch::channel(StdinState::Open);
        let (_stdout_error_tx, mut stdout_errors) = mpsc::channel(1);
        let (_socket_error_tx, mut socket_errors) = mpsc::channel(1);
        let payload = vec![b'x'; IO_CHUNK_BYTES * 3];
        let mut socket_write_closed = false;

        let mut enqueue = Box::pin(enqueue_chunks(
            &sender,
            &payload,
            &mut stdin_state,
            &mut stdout_errors,
            &mut socket_errors,
            &mut socket_write_closed,
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert!(matches!(enqueue.as_mut().poll(&mut context), Poll::Pending));
        let mut received = Vec::new();
        received.extend(receiver.try_recv().expect("first queued chunk"));
        assert!(matches!(enqueue.as_mut().poll(&mut context), Poll::Pending));
        received.extend(receiver.try_recv().expect("second queued chunk"));
        assert!(matches!(
            enqueue.as_mut().poll(&mut context),
            Poll::Ready(EnqueueResult::Complete)
        ));
        received.extend(receiver.try_recv().expect("third queued chunk"));
        assert_eq!(received, payload);
    }

    #[tokio::test]
    async fn enqueue_capacity_wait_is_interrupted_by_stdin_eof() {
        let (sender, _receiver) = mpsc::channel(1);
        let (stdin_tx, mut stdin_state) = watch::channel(StdinState::Open);
        let (_stdout_error_tx, mut stdout_errors) = mpsc::channel(1);
        let (_socket_error_tx, mut socket_errors) = mpsc::channel(1);
        let payload = vec![b'x'; IO_CHUNK_BYTES * 2];
        let mut socket_write_closed = false;
        let mut enqueue = Box::pin(enqueue_chunks(
            &sender,
            &payload,
            &mut stdin_state,
            &mut stdout_errors,
            &mut socket_errors,
            &mut socket_write_closed,
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert!(matches!(enqueue.as_mut().poll(&mut context), Poll::Pending));
        stdin_tx.send_replace(StdinState::Eof);
        assert!(matches!(
            enqueue.as_mut().poll(&mut context),
            Poll::Ready(EnqueueResult::StdinExit(0))
        ));
    }

    #[tokio::test]
    async fn finish_stdout_waits_for_drain_completion() {
        let (sender, mut receiver) = mpsc::channel(1);
        let pending_output = b"final response\n".to_vec();
        sender.try_send(pending_output.clone()).unwrap();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (_stdout_error_tx, mut stdout_errors) = mpsc::channel(1);
        let (_stdin_tx, mut stdin_state) = watch::channel(StdinState::Open);
        let mut waiting = Box::pin(finish_stdout(
            sender,
            done_rx,
            &mut stdout_errors,
            &mut stdin_state,
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(receiver.try_recv().unwrap(), pending_output);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        done_tx.send(()).unwrap();
        assert!(matches!(
            waiting.as_mut().poll(&mut context),
            Poll::Ready(0)
        ));
    }
}
