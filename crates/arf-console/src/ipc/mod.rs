//! IPC server for external tool access to the arf R session.
//!
//! Provides a JSON-RPC 2.0 interface over Unix sockets (or named pipes on Windows)
//! for AI agents, vscode-R, and other tools to interact with R.
//!
//! ## Mutual exclusion between console and IPC input
//!
//! All IPC operations that could conflict with console input (user_input,
//! evaluate with visible=true/false) are routed through reedline's ExternalBreak
//! mechanism. This allows the REPL to check the editor buffer before accepting
//! or rejecting the operation:
//!
//! 1. Idle callback receives IPC request → stores in `PENDING_IPC_OPERATION` → fires break signal
//! 2. Reedline returns `Signal::ExternalBreak(buffer)` with current editor buffer
//! 3. REPL checks buffer: empty → accept operation, non-empty → reject with `USER_IS_TYPING`

mod capture;
pub mod client;
pub mod protocol;
pub mod server;
pub mod session;

use protocol::{
    EvaluateResult, INPUT_ALREADY_PENDING, IpcMethod, IpcRequest, IpcResponse, R_BUSY,
    R_NOT_AT_PROMPT, USER_IS_TYPING, UserInputResult,
};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// Shutdown flag for headless mode. When set, the headless event loop exits.
/// Not set in REPL mode (shutdown is only available in headless mode).
static HEADLESS_SHUTDOWN: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Channel receiver for IPC requests (server → main thread).
/// Wrapped in Option so it can be replaced on restart.
static IPC_RECEIVER: OnceLock<Mutex<Option<std::sync::mpsc::Receiver<IpcRequest>>>> =
    OnceLock::new();

/// Pending IPC operation waiting for ExternalBreak to check the editor buffer.
/// At most one operation can be pending at a time.
static PENDING_IPC_OPERATION: OnceLock<Mutex<Option<PendingIpcOperation>>> = OnceLock::new();

fn pending_ipc_operation() -> &'static Mutex<Option<PendingIpcOperation>> {
    PENDING_IPC_OPERATION.get_or_init(|| Mutex::new(None))
}

/// Whether R is currently busy evaluating (not waiting for input).
static R_IS_AT_PROMPT: OnceLock<AtomicBool> = OnceLock::new();

/// Whether the REPL is in an alternate mode (shell mode, history browser, help browser)
/// where the idle callback is not running and IPC requests would hang.
static IN_ALTERNATE_MODE: AtomicBool = AtomicBool::new(false);

fn r_is_at_prompt() -> &'static AtomicBool {
    R_IS_AT_PROMPT.get_or_init(|| AtomicBool::new(false))
}

/// Set whether the REPL is in an alternate mode (shell, history browser, etc.).
pub fn set_in_alternate_mode(active: bool) {
    IN_ALTERNATE_MODE.store(active, Ordering::Release);
}

/// Check if the REPL is in an alternate mode.
pub fn is_in_alternate_mode() -> bool {
    IN_ALTERNATE_MODE.load(Ordering::Acquire)
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

/// An IPC operation waiting for ExternalBreak buffer check.
pub struct PendingIpcOperation {
    pub kind: PendingIpcKind,
    pub code: String,
}

/// The kind of pending IPC operation, carrying its reply channel.
pub enum PendingIpcKind {
    /// Silent evaluate: run R code in the REPL thread with output capture.
    /// Reply is sent immediately after evaluation completes.
    SilentEvaluate {
        reply: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    /// Visible evaluate: inject code into REPL, capture output via WriteConsoleEx.
    /// Reply is deferred until R returns to the prompt.
    VisibleEvaluate {
        reply: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    /// User input: inject code into REPL as if the user typed it.
    /// Reply is sent when the operation is accepted or rejected.
    UserInput {
        reply: tokio::sync::oneshot::Sender<IpcResponse>,
    },
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

    // Initialize pending operation storage
    let _ = pending_ipc_operation();

    // Start the server thread first; only update the receiver after
    // confirming that the server bound successfully, so a failed start
    // doesn't break an already-running server's channel.
    let path = server::start_server(tx)?;

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

    Ok(path)
}

/// Stop the IPC server (cleanup on exit).
///
/// Clears any in-flight IPC state (pending operations, active capture)
/// so the REPL doesn't get stuck in a pending/capturing state.
pub fn stop_server() {
    // Drop the receiver so the server thread's mpsc::send fails
    if let Some(receiver) = IPC_RECEIVER.get() {
        *receiver.lock().unwrap() = None;
    }

    // Reply to any pending operation with a cancellation error
    if let Some(pending) = take_pending_ipc_operation() {
        match pending.kind {
            PendingIpcKind::SilentEvaluate { reply }
            | PendingIpcKind::VisibleEvaluate { reply }
            | PendingIpcKind::UserInput { reply } => {
                let _ = reply.send(IpcResponse::Error {
                    code: R_NOT_AT_PROMPT,
                    message: "IPC server is shutting down".to_string(),
                });
            }
        }
    }

    // Finalize any active visible eval capture.
    // Use blocking lock — stop_server is a final cleanup and can afford to wait.
    if let Some(pending) = pending_visible_eval()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let (stdout, stderr) = arf_libr::finish_ipc_capture();
        let _ = pending.reply.send(IpcResponse::Error {
            code: R_NOT_AT_PROMPT,
            message: format!(
                "IPC server shut down during visible evaluate (stdout: {} bytes, stderr: {} bytes)",
                stdout.len(),
                stderr.len(),
            ),
        });
    }

    server::stop_server();
}

/// Poll for and handle IPC requests.
///
/// Called from the reedline idle callback (~33ms interval).
/// Requests are not processed immediately — they are stored in
/// `PENDING_IPC_OPERATION` and the break signal is fired so that
/// the ExternalBreak handler can check the editor buffer first.
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
    // Check prompt state for normal completion, and elapsed time for timeout cleanup.
    let at_prompt = r_is_at_prompt().load(Ordering::Acquire);

    let mut guard = match pending_visible_eval().try_lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    // Check for staleness/timeout regardless of prompt state.
    // NOTE: In the timeout path, R may still be actively evaluating the injected code.
    // Calling finish_ipc_capture() here races with R's WriteConsoleEx calls — some output
    // may be lost or partial. This is a best-effort cleanup to avoid indefinite hangs;
    // subsequent R output from the timed-out evaluation will go to the default handler.
    if let Some(pending) = guard.as_ref()
        && pending.started_at.elapsed() > VISIBLE_EVAL_TIMEOUT
    {
        let pending = guard.take().unwrap();
        // Best-effort capture cleanup (may race with active R evaluation)
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
        // Do NOT set r_is_at_prompt(true) here — R may still be evaluating.
        // The flag will be set when R actually returns to the prompt via
        // set_r_at_prompt(true) in the normal REPL flow.
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
///
/// Instead of processing requests directly, stores them in `PENDING_IPC_OPERATION`
/// and fires the break signal. The actual processing happens in the ExternalBreak
/// handler in the REPL, which can check the editor buffer for mutual exclusion.
fn handle_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;

    // Reject if in alternate mode (shell, history browser, help browser).
    // Normally dispatch_request() catches this first, but this covers the
    // race where a request was queued just before alternate mode was entered.
    if is_in_alternate_mode() {
        let _ = reply.send(IpcResponse::Error {
            code: R_NOT_AT_PROMPT,
            message: "R is not at the command prompt".to_string(),
        });
        return;
    }

    // Reject if there's already a pending operation waiting for ExternalBreak.
    // Uses INPUT_ALREADY_PENDING (not R_BUSY) so clients can distinguish
    // "R is evaluating" from "another IPC request is queued."
    if pending_ipc_operation()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        let _ = reply.send(IpcResponse::Error {
            code: INPUT_ALREADY_PENDING,
            message: "Another IPC operation is pending".to_string(),
        });
        return;
    }

    match method {
        IpcMethod::Evaluate { code, visible } => {
            // Check if R is at the prompt (idle)
            if !r_is_at_prompt().load(Ordering::Acquire) {
                let _ = reply.send(IpcResponse::Error {
                    code: R_BUSY,
                    message: "R is busy".to_string(),
                });
                return;
            }

            let kind = if visible {
                PendingIpcKind::VisibleEvaluate { reply }
            } else {
                PendingIpcKind::SilentEvaluate { reply }
            };

            // Store operation and fire break signal
            *pending_ipc_operation()
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(PendingIpcOperation { kind, code });
            fire_break_signal();
        }
        IpcMethod::UserInput { code } => {
            // Check if R is at the prompt
            if !r_is_at_prompt().load(Ordering::Acquire) {
                let _ = reply.send(IpcResponse::Error {
                    code: R_NOT_AT_PROMPT,
                    message: "R is not at the command prompt".to_string(),
                });
                return;
            }

            // Store operation and fire break signal
            *pending_ipc_operation()
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(PendingIpcOperation {
                kind: PendingIpcKind::UserInput { reply },
                code,
            });
            fire_break_signal();
        }
    }
}

/// Fire the break signal to interrupt reedline's `read_line()` loop.
fn fire_break_signal() {
    if let Some(signal) = BREAK_SIGNAL.get() {
        signal.store(true, Ordering::Relaxed);
    }
}

// ── Public API for the REPL ──────────────────────────────────────────────

/// Take the pending IPC operation (if any).
///
/// Called from the REPL's ExternalBreak handler and fast-path to process
/// the stored operation after checking the editor buffer.
pub fn take_pending_ipc_operation() -> Option<PendingIpcOperation> {
    pending_ipc_operation()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// Reject a pending IPC operation because the user is typing.
///
/// Sends a `USER_IS_TYPING` error response to the IPC client.
pub fn reject_operation_user_typing(op: PendingIpcOperation) {
    let message = "User is typing in the console".to_string();
    match op.kind {
        PendingIpcKind::SilentEvaluate { reply }
        | PendingIpcKind::VisibleEvaluate { reply }
        | PendingIpcKind::UserInput { reply } => {
            let _ = reply.send(IpcResponse::Error {
                code: USER_IS_TYPING,
                message,
            });
        }
    }
}

/// Set up a visible evaluate: start capture and store the deferred reply.
///
/// Called from the REPL after the buffer check passes. The reply will be
/// sent later by `check_visible_eval_completion` when R returns to the prompt.
pub fn setup_visible_eval(reply: tokio::sync::oneshot::Sender<IpcResponse>) {
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
}

/// Run a silent evaluate and send the reply.
///
/// Called from the REPL after the buffer check passes. Runs R code with
/// full output capture (stdout/stderr via WriteConsoleEx, value/error via
/// temp file). The response is sent synchronously before returning.
///
/// **Note:** `evaluate_with_capture` does not support interactive R functions
/// (e.g., `readline()`, `browser()`, `menu()`). If the evaluated code triggers
/// a nested `ReadConsole` callback, R will block waiting for input that never
/// arrives, eventually requiring user intervention from the console.
pub fn run_silent_eval(code: &str, reply: tokio::sync::oneshot::Sender<IpcResponse>) {
    r_is_at_prompt().store(false, Ordering::Release);

    let result = capture::evaluate_with_capture(code, false);

    r_is_at_prompt().store(true, Ordering::Release);
    let _ = reply.send(IpcResponse::Evaluate(result));
}

/// Accept a user_input operation: send the success reply.
pub fn accept_user_input(reply: tokio::sync::oneshot::Sender<IpcResponse>) {
    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
}

/// Mark that R is now at the command prompt (idle, ready for input).
pub fn set_r_at_prompt(at_prompt: bool) {
    r_is_at_prompt().store(at_prompt, Ordering::Release);
}

/// Register the headless shutdown flag so `shutdown` IPC method can trigger it.
///
/// Must be called before `start_server()` in headless mode. In REPL mode
/// this is never called, so `shutdown` requests return METHOD_NOT_FOUND.
pub fn set_headless_shutdown(flag: Arc<AtomicBool>) {
    let _ = HEADLESS_SHUTDOWN.set(flag);
}

/// Try to trigger headless shutdown. Returns true if the flag was set.
///
/// Called from the server thread when a `shutdown` request arrives.
/// Returns false if not in headless mode (flag not registered).
pub fn trigger_headless_shutdown() -> bool {
    if let Some(flag) = HEADLESS_SHUTDOWN.get() {
        flag.store(true, Ordering::Release);
        true
    } else {
        false
    }
}

// ── Headless mode API ────────────────────────────────────────────────────

/// Poll and directly process IPC requests in headless mode.
///
/// Unlike `poll_ipc_requests` (used by the REPL), this function processes
/// requests immediately without the ExternalBreak/editor-buffer check,
/// since there is no interactive editor in headless mode.
///
/// Returns `true` if at least one request was processed.
pub fn headless_poll_and_process() -> bool {
    let receiver = match IPC_RECEIVER.get() {
        Some(r) => r,
        None => return false,
    };

    let rx = match receiver.try_lock() {
        Ok(rx) => rx,
        Err(_) => return false,
    };

    let rx = match rx.as_ref() {
        Some(rx) => rx,
        None => return false,
    };

    let mut processed = false;

    while let Ok(request) = rx.try_recv() {
        processed = true;
        headless_handle_request(request);
    }

    processed
}

/// Handle a single IPC request in headless mode.
///
/// Processes evaluate and user_input requests directly on the R thread.
/// No editor buffer check is needed since there is no interactive console.
fn headless_handle_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;

    match method {
        IpcMethod::Evaluate { code, visible } => {
            // When visible=true, captured output is also written to the
            // headless process's stdout/stderr for logging/monitoring.
            r_is_at_prompt().store(false, Ordering::Release);
            let result = capture::evaluate_with_capture(&code, visible);
            r_is_at_prompt().store(true, Ordering::Release);
            let _ = reply.send(IpcResponse::Evaluate(result));
        }
        IpcMethod::UserInput { code } => {
            // In headless mode, user_input evaluates the code directly.
            // Output goes to the default WriteConsoleEx handler (stdout/stderr).
            r_is_at_prompt().store(false, Ordering::Release);
            let eval_result = arf_harp::eval_string(&code);
            r_is_at_prompt().store(true, Ordering::Release);
            match eval_result {
                Ok(_) => {
                    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
                }
                Err(e) => {
                    log::warn!("Headless user_input evaluation error: {}", e);
                    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: false }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests for the IN_ALTERNATE_MODE flag and handle_request rejection.
    ///
    /// Combined into a single test to avoid flakiness from parallel test
    /// execution, since all tests share the global `IN_ALTERNATE_MODE` atomic.
    #[test]
    fn test_alternate_mode_flag_and_request_rejection() {
        // Reset global state first to avoid interference from parallel tests
        set_in_alternate_mode(false);
        set_r_at_prompt(false);

        // Should be false after reset
        assert!(!is_in_alternate_mode());

        // Toggle on/off
        set_in_alternate_mode(true);
        assert!(is_in_alternate_mode());
        set_in_alternate_mode(false);
        assert!(!is_in_alternate_mode());

        // Set R at prompt so we test alternate mode rejection specifically
        // (not the R_BUSY / R_NOT_AT_PROMPT check that comes after)
        set_r_at_prompt(true);
        set_in_alternate_mode(true);
        {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let request = IpcRequest {
                method: IpcMethod::UserInput {
                    code: "1+1".to_string(),
                },
                reply: reply_tx,
            };
            handle_request(request);
            match reply_rx.blocking_recv().unwrap() {
                IpcResponse::Error { code, .. } => assert_eq!(code, R_NOT_AT_PROMPT),
                _ => panic!("Expected R_NOT_AT_PROMPT error for user_input"),
            }
        }

        // handle_request rejects evaluate in alternate mode
        {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let request = IpcRequest {
                method: IpcMethod::Evaluate {
                    code: "1+1".to_string(),
                    visible: false,
                },
                reply: reply_tx,
            };
            handle_request(request);
            match reply_rx.blocking_recv().unwrap() {
                IpcResponse::Error { code, .. } => assert_eq!(code, R_NOT_AT_PROMPT),
                _ => panic!("Expected R_NOT_AT_PROMPT error for evaluate"),
            }
        }

        // Cleanup
        set_in_alternate_mode(false);
        set_r_at_prompt(false);
    }
}
