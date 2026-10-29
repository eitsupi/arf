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
    Overflow,
    ReadError(String),
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
    std::thread::Builder::new()
        .name("arf-ipc-stdout".into())
        .spawn(move || stdout_worker(stdout_rx, stdout_error_tx))
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
        "id": 1,
        "method": "attach",
        "params": { "subscribe": subscribe },
    });
    let mut attach_line = serde_json::to_vec(&attach).expect("serializing JSON value cannot fail");
    attach_line.push(b'\n');
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
                return;
            }
        }
    });

    // Keep the buffer across select! iterations. This preserves any bytes
    // read beyond the attach response and makes the read cancellation-safe.
    let mut initial = Vec::with_capacity(IO_CHUNK_BYTES);
    let attach_response = loop {
        if let Some(line) = take_first_line(&mut initial) {
            break line;
        }
        if initial.len() > MAX_LINE_BYTES {
            client::exit_error(
                2,
                "PROTOCOL_ERROR",
                "Attach response exceeded the line limit",
                None,
                None,
            );
        }
        tokio::select! {
            result = reader.read_buf(&mut initial) => match result {
                Ok(0) => return 0,
                Ok(_) => {},
                Err(error) => return transport_error(error),
            },
            _ = wait_for_stdin_exit(&mut stdin_state_rx) => return stdin_exit_code(&stdin_state_rx),
            error = stdout_error_rx.recv() => if let Some(error) = error {
                client::exit_error(2, "OUTPUT_ERROR", &error, None, None);
            },
            error = socket_error_rx.recv() => if let Some(error) = error {
                return transport_error(error);
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
    if !enqueue_chunks(&stdout_tx, &attach_response) {
        client::exit_error(
            2,
            "TRANSPORT_ERROR",
            "Bridge output queue is full",
            None,
            None,
        );
    }
    let read_ahead = std::mem::take(&mut initial);
    if !enqueue_chunks(&stdout_tx, &read_ahead) {
        client::exit_error(
            2,
            "TRANSPORT_ERROR",
            "Bridge output queue is full",
            None,
            None,
        );
    }

    let mut chunk = vec![0; IO_CHUNK_BYTES];
    loop {
        tokio::select! {
            biased;
            _ = wait_for_stdin_exit(&mut stdin_state_rx) => return stdin_exit_code(&stdin_state_rx),
            error = stdout_error_rx.recv() => if let Some(error) = error {
                client::exit_error(2, "OUTPUT_ERROR", &error, None, None);
            },
            error = socket_error_rx.recv() => if let Some(error) = error {
                return transport_error(error);
            },
            result = reader.read(&mut chunk) => match result {
                Ok(0) => return 0,
                Ok(length) => {
                    if !enqueue_chunks(&stdout_tx, &chunk[..length]) {
                        client::exit_error(2, "TRANSPORT_ERROR", "Bridge output queue is full", None, None);
                    }
                }
                Err(error) => return transport_error(error),
            },
        }
    }
}

fn take_first_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = buffer.iter().position(|byte| *byte == b'\n')?;
    Some(buffer.drain(..=end).collect())
}

fn enqueue_chunks(sender: &mpsc::Sender<Vec<u8>>, bytes: &[u8]) -> bool {
    for chunk in bytes.chunks(IO_CHUNK_BYTES) {
        if sender.try_send(chunk.to_vec()).is_err() {
            return false;
        }
    }
    true
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
        StdinState::Overflow => client::exit_error(
            2,
            "TRANSPORT_ERROR",
            "Bridge input queue is full",
            None,
            None,
        ),
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
                if sender.try_send(buffer[..length].to_vec()).is_err() {
                    state.send_replace(StdinState::Overflow);
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
    #[cfg(windows)]
    if error.raw_os_error() == Some(109) {
        return 0;
    }
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        return 0;
    }
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
    use super::take_first_line;

    #[test]
    fn attach_line_extraction_retains_partial_and_coalesced_read_ahead() {
        let mut buffered = br#"{"result":{}}"#.to_vec();
        assert!(take_first_line(&mut buffered).is_none());
        buffered.extend_from_slice(b"\nevent\nnext");
        assert_eq!(
            take_first_line(&mut buffered).unwrap(),
            concat!(r#"{"result":{}}"#, "\n").as_bytes()
        );
        assert_eq!(buffered, b"event\nnext");
    }
}
