//! IPC server for external tool access to the arf R session.
//!
//! Provides a JSON-RPC 2.0 interface over Unix sockets (or TCP on Windows)
//! for AI agents, vscode-R, and other tools to interact with R.

mod capture;
pub mod client;
pub mod protocol;
pub mod server;
pub mod session;

use protocol::{
    INPUT_ALREADY_PENDING, IpcMethod, IpcRequest, IpcResponse, R_BUSY, R_NOT_AT_PROMPT,
    UserInputResult,
};
use std::sync::{Mutex, OnceLock};

/// Channel receiver for IPC requests (server → main thread).
static IPC_RECEIVER: OnceLock<Mutex<std::sync::mpsc::Receiver<IpcRequest>>> = OnceLock::new();

/// Pending user input from IPC `user_input` method.
/// Consumed by `read_console_callback` on the next R command prompt.
static IPC_PENDING_INPUT: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// Whether R is currently busy evaluating (not waiting for input).
static R_IS_AT_PROMPT: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();

fn r_is_at_prompt() -> &'static std::sync::atomic::AtomicBool {
    R_IS_AT_PROMPT.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}

/// Start the IPC server and set up channels.
///
/// Called from `main.rs` when `--with-ipc` is specified.
pub fn start_server() -> std::io::Result<String> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Store receiver for polling from idle callback
    IPC_RECEIVER
        .set(Mutex::new(rx))
        .map_err(|_| std::io::Error::other("IPC already initialized"))?;

    // Initialize pending input storage
    let _ = IPC_PENDING_INPUT.get_or_init(|| Mutex::new(None));

    server::start_server(tx)
}

/// Stop the IPC server (cleanup on exit).
pub fn stop_server() {
    server::stop_server();
}

/// Poll for and handle IPC requests.
///
/// Called from the reedline idle callback (~33ms interval).
/// Only processes `evaluate` requests here; `user_input` is handled
/// by storing the code for the next `read_console_callback`.
pub fn poll_ipc_requests() {
    let receiver = match IPC_RECEIVER.get() {
        Some(r) => r,
        None => return, // IPC not started
    };

    let rx = match receiver.try_lock() {
        Ok(rx) => rx,
        Err(_) => return, // Already locked (shouldn't happen in single-threaded context)
    };

    // Process all pending requests
    while let Ok(request) = rx.try_recv() {
        handle_request(request);
    }
}

/// Handle a single IPC request on the main thread.
fn handle_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;

    match method {
        IpcMethod::Evaluate { code } => {
            // Check if R is at the prompt (idle)
            if !r_is_at_prompt().load(std::sync::atomic::Ordering::Relaxed) {
                let _ = reply.send(IpcResponse::Error {
                    code: R_BUSY,
                    message: "R is busy".to_string(),
                });
                return;
            }

            // Evaluate with output capture
            let result = capture::evaluate_with_capture(&code);
            let _ = reply.send(IpcResponse::Evaluate(result));
        }
        IpcMethod::UserInput { code } => {
            // Check if R is at the prompt
            if !r_is_at_prompt().load(std::sync::atomic::Ordering::Relaxed) {
                let _ = reply.send(IpcResponse::Error {
                    code: R_NOT_AT_PROMPT,
                    message: "R is not at the command prompt".to_string(),
                });
                return;
            }

            // Check if there's already pending input
            let pending = IPC_PENDING_INPUT.get_or_init(|| Mutex::new(None));
            let mut guard = match pending.try_lock() {
                Ok(g) => g,
                Err(_) => {
                    let _ = reply.send(IpcResponse::Error {
                        code: INPUT_ALREADY_PENDING,
                        message: "Input already pending".to_string(),
                    });
                    return;
                }
            };

            if guard.is_some() {
                let _ = reply.send(IpcResponse::Error {
                    code: INPUT_ALREADY_PENDING,
                    message: "Previous input not yet consumed".to_string(),
                });
                return;
            }

            *guard = Some(code);
            let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
        }
    }
}

/// Take pending IPC input (if any).
///
/// Called from `read_console_callback` to inject IPC-provided input
/// into the R evaluation loop.
pub fn take_ipc_pending_input() -> Option<String> {
    let pending = IPC_PENDING_INPUT.get()?;
    let mut guard = pending.try_lock().ok()?;
    guard.take()
}

/// Mark that R is now at the command prompt (idle, ready for input).
pub fn set_r_at_prompt(at_prompt: bool) {
    r_is_at_prompt().store(at_prompt, std::sync::atomic::Ordering::Relaxed);
}
