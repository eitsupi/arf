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
    EvaluateResult, INPUT_ALREADY_PENDING, IpcMethod, IpcRequest, IpcResponse, R_BUSY,
    R_NOT_AT_PROMPT, UserInputResult,
};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// Channel receiver for IPC requests (server → main thread).
/// Wrapped in Option so it can be replaced on restart.
static IPC_RECEIVER: OnceLock<Mutex<Option<std::sync::mpsc::Receiver<IpcRequest>>>> =
    OnceLock::new();

/// Pending user input from IPC `user_input` method.
/// Consumed by `read_console_callback` on the next R command prompt.
static IPC_PENDING_INPUT: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// Whether R is currently busy evaluating (not waiting for input).
static R_IS_AT_PROMPT: OnceLock<AtomicBool> = OnceLock::new();

fn r_is_at_prompt() -> &'static AtomicBool {
    R_IS_AT_PROMPT.get_or_init(|| AtomicBool::new(false))
}

/// Break signal shared with reedline to interrupt `read_line()`.
static BREAK_SIGNAL: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Pending visible evaluate: reply channel waiting for REPL evaluation to complete.
/// When set, the WriteConsoleEx callback is capturing output. The reply is sent
/// once R returns to the prompt (detected by `check_visible_eval_completion`).
static PENDING_VISIBLE_EVAL: OnceLock<Mutex<Option<PendingVisibleEval>>> = OnceLock::new();

struct PendingVisibleEval {
    reply: tokio::sync::oneshot::Sender<IpcResponse>,
    started_at: std::time::Instant,
}

fn pending_visible_eval() -> &'static Mutex<Option<PendingVisibleEval>> {
    PENDING_VISIBLE_EVAL.get_or_init(|| Mutex::new(None))
}

/// Get or create the break signal for reedline integration.
/// Pass the returned `Arc` to `Reedline::with_break_signal()`.
pub fn break_signal() -> Arc<AtomicBool> {
    BREAK_SIGNAL
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// Start the IPC server and set up channels.
///
/// Called from `main.rs` when `--with-ipc` is specified,
/// or from `:ipc start` meta command.
pub fn start_server() -> std::io::Result<String> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Store receiver for polling from idle callback.
    // If OnceLock is already set (from a previous stop/start), replace the inner value.
    match IPC_RECEIVER.get() {
        Some(existing) => {
            *existing.lock().unwrap() = Some(rx);
        }
        None => {
            IPC_RECEIVER
                .set(Mutex::new(Some(rx)))
                .map_err(|_| std::io::Error::other("IPC receiver already initialized"))?;
        }
    }

    // Initialize pending input storage
    let _ = IPC_PENDING_INPUT.get_or_init(|| Mutex::new(None));

    server::start_server(tx)
}

/// Stop the IPC server (cleanup on exit).
pub fn stop_server() {
    // Drop the receiver so the server thread's mpsc::send fails
    if let Some(receiver) = IPC_RECEIVER.get() {
        *receiver.lock().unwrap() = None;
    }
    server::stop_server();
}

/// Poll for and handle IPC requests.
///
/// Called from the reedline idle callback (~33ms interval).
/// Only processes `evaluate` requests here; `user_input` is handled
/// by storing the code for the next `read_console_callback`.
pub fn poll_ipc_requests() {
    // Check if a visible evaluate has completed (R returned to prompt)
    check_visible_eval_completion();

    let receiver = match IPC_RECEIVER.get() {
        Some(r) => r,
        None => return, // IPC not started
    };

    let rx = match receiver.try_lock() {
        Ok(rx) => rx,
        Err(_) => return,
    };

    let rx = match rx.as_ref() {
        Some(rx) => rx,
        None => return, // IPC stopped
    };

    // Process all pending requests
    while let Ok(request) = rx.try_recv() {
        handle_request(request);
    }
}

/// Check if a pending visible evaluate has completed.
///
/// A visible evaluate injects code into the REPL (like user_input) and waits
/// for the result. When R returns to the prompt, the evaluation is done and
/// we can collect captured output and send the reply.
/// Timeout for visible evaluate: if R hasn't returned to the prompt within this
/// duration, we assume something went wrong and send an error response.
/// Matches the client-side timeout.
const VISIBLE_EVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

fn check_visible_eval_completion() {
    // Only complete when R is back at the prompt
    let at_prompt = r_is_at_prompt().load(Ordering::Acquire);

    let mut guard = match pending_visible_eval().try_lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    // Check for staleness/timeout regardless of prompt state
    if let Some(pending) = guard.as_ref()
        && pending.started_at.elapsed() > VISIBLE_EVAL_TIMEOUT
    {
        let pending = guard.take().unwrap();
        // Clean up capture state
        let (stdout, stderr) = arf_libr::finish_ipc_capture();
        let _ = pending.reply.send(IpcResponse::Error {
            code: R_BUSY,
            message: format!(
                "Visible evaluate timed out after {}s (stdout: {} bytes, stderr: {} bytes)",
                VISIBLE_EVAL_TIMEOUT.as_secs(),
                stdout.len(),
                stderr.len(),
            ),
        });
        r_is_at_prompt().store(true, Ordering::Release);
        return;
    }

    if !at_prompt {
        return;
    }

    if let Some(pending) = guard.take() {
        // Prevent new requests from starting during capture finalization
        r_is_at_prompt().store(false, Ordering::Release);

        // Finish WriteConsoleEx capture and collect output
        let (stdout, stderr) = arf_libr::finish_ipc_capture();

        let result = EvaluateResult {
            stdout,
            stderr,
            // In visible mode, auto-printed values are in stdout and errors in stderr.
            // Structured value/error fields are not available because the code runs
            // through normal REPL evaluation (no tryCatch wrapper).
            value: None,
            error: None,
        };

        let _ = pending.reply.send(IpcResponse::Evaluate(result));

        // Restore prompt state now that finalization is complete
        r_is_at_prompt().store(true, Ordering::Release);
    }
}

/// Handle a single IPC request on the main thread.
fn handle_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;

    match method {
        IpcMethod::Evaluate { code, visible } => {
            // Check if R is at the prompt (idle)
            if !r_is_at_prompt().load(Ordering::Relaxed) {
                let _ = reply.send(IpcResponse::Error {
                    code: R_BUSY,
                    message: "R is busy".to_string(),
                });
                return;
            }

            if visible {
                // Visible evaluate: inject code into REPL and capture output.
                // The code appears at the prompt, R evaluates it normally, and
                // output is both shown in the terminal AND captured for the response.
                handle_visible_evaluate(code, reply);
            } else {
                // Silent evaluate: run in idle callback with full capture
                r_is_at_prompt().store(false, Ordering::Relaxed);

                let result = capture::evaluate_with_capture(&code);

                r_is_at_prompt().store(true, Ordering::Relaxed);
                let _ = reply.send(IpcResponse::Evaluate(result));
            }
        }
        IpcMethod::UserInput { code } => {
            // Check if R is at the prompt
            if !r_is_at_prompt().load(Ordering::Relaxed) {
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

            // Fire the break signal to interrupt reedline's read_line() loop.
            // This causes read_line() to return Signal::ExternalBreak, allowing
            // the REPL to consume the pending input.
            if let Some(signal) = BREAK_SIGNAL.get() {
                signal.store(true, Ordering::Relaxed);
            }

            let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
        }
    }
}

/// Handle a visible evaluate request.
///
/// Instead of evaluating in the idle callback, we inject the code into the REPL
/// (same as user_input) and start the WriteConsoleEx capture. The reply is deferred
/// until R returns to the prompt, at which point `check_visible_eval_completion`
/// collects the captured output and sends the response.
fn handle_visible_evaluate(code: String, reply: tokio::sync::oneshot::Sender<IpcResponse>) {
    // Check if there's already pending input
    let pending_input = IPC_PENDING_INPUT.get_or_init(|| Mutex::new(None));
    let mut input_guard = match pending_input.try_lock() {
        Ok(g) => g,
        Err(_) => {
            let _ = reply.send(IpcResponse::Error {
                code: INPUT_ALREADY_PENDING,
                message: "Input already pending".to_string(),
            });
            return;
        }
    };

    if input_guard.is_some() {
        let _ = reply.send(IpcResponse::Error {
            code: INPUT_ALREADY_PENDING,
            message: "Previous input not yet consumed".to_string(),
        });
        return;
    }

    // Mark R as busy to reject concurrent requests
    r_is_at_prompt().store(false, Ordering::Release);

    // Start WriteConsoleEx capture (visible=true → also print to terminal)
    arf_libr::start_ipc_capture(true);

    // Store reply channel — will be consumed by check_visible_eval_completion
    *pending_visible_eval()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(PendingVisibleEval {
        reply,
        started_at: std::time::Instant::now(),
    });

    // Inject code into REPL
    *input_guard = Some(code);

    // Fire break signal to interrupt read_line()
    if let Some(signal) = BREAK_SIGNAL.get() {
        signal.store(true, Ordering::Relaxed);
    }

    // Reply is NOT sent here — deferred until evaluation completes
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
    r_is_at_prompt().store(at_prompt, Ordering::Release);
}
