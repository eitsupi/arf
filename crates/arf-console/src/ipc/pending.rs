//! REPL-side handling of IPC requests.
//!
//! Requests that could conflict with console input are stored as a single
//! [`PendingIpcOperation`] and the REPL is interrupted so it can check the
//! editor buffer before accepting or rejecting the operation.

use super::execution;
use super::protocol::{
    INPUT_ALREADY_PENDING, IpcMethod, IpcRequest, IpcResponse, R_BUSY, R_NOT_AT_PROMPT,
    USER_IS_TYPING,
};
use super::session_result::collect_session_result;
use super::state::{drain_requests, fire_break_signal, is_in_alternate_mode, r_is_at_prompt};
use crate::sync::LockRecover;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock, atomic::Ordering};

/// Calls can run without touching the editor buffer. A barrier keeps an
/// accepted legacy operation in FIFO order while preserving its single-slot
/// admission rule. Later calls wait until that operation has started.
#[derive(Default)]
struct InteractiveCalls {
    requests: VecDeque<IpcRequest>,
    before_pending: Option<usize>,
}

static INTERACTIVE_CALLS: OnceLock<Mutex<InteractiveCalls>> = OnceLock::new();

fn interactive_calls() -> &'static Mutex<InteractiveCalls> {
    INTERACTIVE_CALLS.get_or_init(|| Mutex::new(InteractiveCalls::default()))
}

fn schedule_pending_operation() {
    let mut calls = interactive_calls().lock_recover();
    calls.before_pending = Some(calls.requests.len());
    if calls.requests.is_empty() {
        fire_break_signal();
    }
}

/// Run at most one handler per idle callback, keeping reedline's buffer and
/// history intact and returning to terminal input between handler calls.
fn process_interactive_call() {
    if !r_is_at_prompt().load(Ordering::Acquire) || is_in_alternate_mode() {
        return;
    }
    let request = {
        let mut calls = interactive_calls().lock_recover();
        if calls.before_pending == Some(0) {
            fire_break_signal();
            return;
        }
        let request = calls.requests.pop_front();
        if request.is_some()
            && let Some(before) = calls.before_pending.as_mut()
        {
            *before -= 1;
        }
        request
    };
    if let Some(request) = request {
        super::handlers::execute_handler_request(request);
    }
}

/// Pending IPC operation waiting for ExternalBreak to check the editor buffer.
/// At most one operation can be pending at a time.
static PENDING_IPC_OPERATION: OnceLock<Mutex<Option<PendingIpcOperation>>> = OnceLock::new();

pub(super) fn pending_ipc_operation() -> &'static Mutex<Option<PendingIpcOperation>> {
    PENDING_IPC_OPERATION.get_or_init(|| Mutex::new(None))
}

/// Pending visible evaluate: reply channel waiting for REPL evaluation to complete.
/// When set, the WriteConsoleEx callback is capturing output. The reply is sent
/// once R returns to the prompt (detected by `check_visible_eval_completion`).
static PENDING_VISIBLE_EVAL: OnceLock<Mutex<Option<PendingVisibleEval>>> = OnceLock::new();

struct PendingVisibleEval {
    reply: tokio::sync::oneshot::Sender<IpcResponse>,
    execution_id: super::events::ExecutionId,
}

fn pending_visible_eval() -> &'static Mutex<Option<PendingVisibleEval>> {
    PENDING_VISIBLE_EVAL.get_or_init(|| Mutex::new(None))
}

/// An IPC operation waiting for ExternalBreak buffer check.
pub struct PendingIpcOperation {
    pub kind: PendingIpcKind,
    pub code: String,
    pub origin: u64,
    pub started_execution_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
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
        timeout: std::time::Duration,
    },
    /// User input: inject code into REPL as if the user typed it.
    /// Reply is sent when the operation is accepted or rejected.
    UserInput {
        reply: tokio::sync::oneshot::Sender<IpcResponse>,
    },
}

impl PendingIpcKind {
    /// Consume the operation kind and return its reply channel.
    fn into_reply(self) -> tokio::sync::oneshot::Sender<IpcResponse> {
        match self {
            PendingIpcKind::SilentEvaluate { reply }
            | PendingIpcKind::VisibleEvaluate { reply, .. }
            | PendingIpcKind::UserInput { reply } => reply,
        }
    }

    pub fn reply_is_closed(&self) -> bool {
        match self {
            PendingIpcKind::SilentEvaluate { reply }
            | PendingIpcKind::VisibleEvaluate { reply, .. }
            | PendingIpcKind::UserInput { reply } => reply.is_closed(),
        }
    }
}

/// Send an error response on a reply channel, ignoring a closed receiver.
pub(super) fn reply_error(
    reply: tokio::sync::oneshot::Sender<IpcResponse>,
    code: i32,
    message: impl Into<String>,
) {
    let _ = reply.send(IpcResponse::error(code, message.into()));
}

/// Reply to every in-flight operation with a shutdown error.
///
/// Called when the IPC server stops so the REPL doesn't get stuck in a
/// pending/capturing state.
pub(super) fn cancel_in_flight_operations() {
    *interactive_calls().lock_recover() = InteractiveCalls::default();
    // Reply to any pending operation with a cancellation error
    if let Some(pending) = take_pending_ipc_operation() {
        reply_error(
            pending.kind.into_reply(),
            R_NOT_AT_PROMPT,
            "IPC server is shutting down",
        );
    }

    // Finalize any active visible eval capture.
    // Use blocking lock — stop_server is a final cleanup and can afford to wait.
    if let Some(pending) = pending_visible_eval().lock_recover().take() {
        let result = execution::finish_visible_eval();
        reply_error(
            pending.reply,
            R_NOT_AT_PROMPT,
            format!(
                "IPC server shut down during visible evaluate (stdout: {} bytes, stderr: {} bytes)",
                result.stdout.len(),
                result.stderr.len(),
            ),
        );
    }
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

    // Process all pending requests
    drain_requests(handle_request);
    process_interactive_call();
}

/// Check if a pending visible evaluate has completed.
///
/// A visible evaluate injects code into the REPL (like user_input) and waits
/// for the result. When R returns to the prompt, the evaluation is done and
/// we can collect captured output and send the reply.
fn check_visible_eval_completion() {
    // Reply deadlines belong to the server. Keep capture until R actually finishes.
    let at_prompt = r_is_at_prompt().load(Ordering::Acquire);

    let mut guard = match pending_visible_eval().try_lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    if !at_prompt {
        return;
    }

    if let Some(pending) = guard.take() {
        // Prevent new requests from starting during capture finalization
        r_is_at_prompt().store(false, Ordering::Release);

        // Finish WriteConsoleEx capture and collect output
        let mut result = execution::finish_visible_eval();
        result.execution_id = Some(pending.execution_id);

        let _ = pending.reply.send(IpcResponse::Evaluate(result));

        // Restore prompt state now that finalization is complete
        r_is_at_prompt().store(true, Ordering::Release);
    }
}

/// Answer a `session` request immediately.
///
/// Session requests are read-only and don't conflict with user input, so they
/// skip ExternalBreak. However, we must not touch R (e.g. via
/// arf_harp::eval_string) unless it is safe: R must be idle, not in alternate
/// mode, and no other IPC operation pending that might race with us.
fn reply_session(reply: tokio::sync::oneshot::Sender<IpcResponse>) {
    let r_at_prompt = r_is_at_prompt().load(Ordering::Acquire);
    let has_pending = pending_ipc_operation().lock_recover().is_some();
    let in_alt_mode = is_in_alternate_mode();

    let try_r = r_at_prompt && !has_pending && !in_alt_mode;
    let reason = if in_alt_mode {
        "R is in alternate mode (shell, history browser, or help browser)"
    } else if !r_at_prompt {
        "R is busy evaluating another expression"
    } else if has_pending {
        "Another IPC operation is pending"
    } else {
        "" // R info will be collected
    };

    let result = collect_session_result(try_r, reason);
    let _ = reply.send(IpcResponse::Session(Box::new(result)));
}

/// Handle a single IPC request on the main thread.
///
/// Instead of processing requests directly, stores them in `PENDING_IPC_OPERATION`
/// and fires the break signal. The actual processing happens in the ExternalBreak
/// handler in the REPL, which can check the editor buffer for mutual exclusion.
fn handle_request(request: IpcRequest) {
    let IpcRequest {
        method,
        reply,
        origin,
        started_execution_id,
    } = request;

    if reply.is_closed() {
        return;
    }

    // Calls wait for a safe top-level prompt, including while the user is
    // typing. They do not consume the legacy pending-operation slot.
    if matches!(method, IpcMethod::Call { .. }) {
        interactive_calls()
            .lock_recover()
            .requests
            .push_back(IpcRequest {
                method,
                reply,
                origin,
                started_execution_id,
            });
        return;
    }

    if matches!(method, IpcMethod::Session) {
        reply_session(reply);
        return;
    }

    // Reject if in alternate mode (shell, history browser, help browser).
    // Normally dispatch_request() catches this first, but this covers the
    // race where a request was queued just before alternate mode was entered.
    if is_in_alternate_mode() {
        reply_error(reply, R_NOT_AT_PROMPT, "R is not at the command prompt");
        return;
    }

    // Reject if there's already a pending operation waiting for ExternalBreak.
    // Uses INPUT_ALREADY_PENDING (not R_BUSY) so clients can distinguish
    // "R is evaluating" from "another IPC request is queued."
    if pending_ipc_operation().lock_recover().is_some() {
        reply_error(
            reply,
            INPUT_ALREADY_PENDING,
            "Another IPC operation is pending",
        );
        return;
    }

    match method {
        IpcMethod::SilentEvaluate { code, .. } => {
            // Check if R is at the prompt (idle)
            if !r_is_at_prompt().load(Ordering::Acquire) {
                reply_error(reply, R_BUSY, "R is busy");
                return;
            }

            // Store operation and fire break signal
            *pending_ipc_operation().lock_recover() = Some(PendingIpcOperation {
                kind: PendingIpcKind::SilentEvaluate { reply },
                code,
                origin,
                started_execution_id,
            });
            schedule_pending_operation();
        }
        IpcMethod::VisibleEvaluate { code, timeout } => {
            if !r_is_at_prompt().load(Ordering::Acquire) {
                reply_error(reply, R_BUSY, "R is busy");
                return;
            }
            *pending_ipc_operation().lock_recover() = Some(PendingIpcOperation {
                kind: PendingIpcKind::VisibleEvaluate { reply, timeout },
                code,
                origin,
                started_execution_id,
            });
            schedule_pending_operation();
        }
        IpcMethod::UserInput { code } => {
            // Check if R is at the prompt
            if !r_is_at_prompt().load(Ordering::Acquire) {
                reply_error(reply, R_NOT_AT_PROMPT, "R is not at the command prompt");
                return;
            }

            // Store operation and fire break signal
            *pending_ipc_operation().lock_recover() = Some(PendingIpcOperation {
                kind: PendingIpcKind::UserInput { reply },
                code,
                origin,
                started_execution_id,
            });
            schedule_pending_operation();
        }
        IpcMethod::Session => unreachable!("Session handled above"),
        IpcMethod::Call { .. } => unreachable!("Calls handled above"),
    }
}

/// Take the pending IPC operation (if any).
///
/// Called from the REPL's ExternalBreak handler and fast-path to process
/// the stored operation after checking the editor buffer.
pub fn take_pending_ipc_operation() -> Option<PendingIpcOperation> {
    let mut calls = interactive_calls().lock_recover();
    if calls.before_pending.is_some_and(|before| before > 0) {
        return None;
    }
    calls.before_pending = None;
    pending_ipc_operation().lock_recover().take()
}

/// Reject a pending IPC operation because the user is typing.
///
/// Sends a `USER_IS_TYPING` error response to the IPC client.
/// The current editor buffer is included in the JSON-RPC error `data`
/// field so that callers (e.g. AI agents) can see what the user is
/// typing and decide whether to retry or abort. The `message` field
/// is kept stable for pattern matching.
///
/// The buffer is capped at 1024 characters to prevent excessively large
/// payloads. When truncated, `buffer_truncated` is `true` and
/// `buffer_original_length` contains the full character count.
pub fn reject_operation_user_typing(op: PendingIpcOperation, buffer: &str) {
    const MAX_BUFFER_CHARS: usize = 1024;
    let original_len = buffer.chars().count();
    let truncated = original_len > MAX_BUFFER_CHARS;
    let preview: String = buffer.chars().take(MAX_BUFFER_CHARS).collect();

    let response = IpcResponse::error_with_data(
        USER_IS_TYPING,
        "User is typing in the console".to_string(),
        serde_json::json!({
            "buffer": preview,
            "buffer_truncated": truncated,
            "buffer_original_length": original_len,
        }),
    );
    let _ = op.kind.into_reply().send(response);
}

/// Set up a visible evaluate: start capture and store the deferred reply.
///
/// Called from the REPL after the buffer check passes. The reply will be
/// sent later by `check_visible_eval_completion` when R returns to the prompt.
pub fn setup_visible_eval(
    reply: tokio::sync::oneshot::Sender<IpcResponse>,
    _timeout: std::time::Duration,
    code: &str,
    origin: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
) {
    let execution_id = execution::begin_visible_eval(code, origin, started_execution_id);

    // Store reply channel — will be consumed by check_visible_eval_completion.
    // A server-side reply timeout never ends this capture or execution context.
    *pending_visible_eval().lock_recover() = Some(PendingVisibleEval {
        reply,
        execution_id,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol;
    use crate::ipc::state::{set_in_alternate_mode, set_r_at_prompt};
    use serial_test::serial;

    /// Drop guard that resets global IPC state on scope exit (including panics).
    struct GlobalStateGuard;

    impl Drop for GlobalStateGuard {
        fn drop(&mut self) {
            set_in_alternate_mode(false);
            set_r_at_prompt(false);
            *interactive_calls().lock_recover() = InteractiveCalls::default();
            pending_ipc_operation().lock_recover().take();
            super::super::state::break_signal().store(false, Ordering::Relaxed);
        }
    }

    fn expired_call() -> (IpcRequest, tokio::sync::oneshot::Receiver<IpcResponse>) {
        let args = serde_json::Value::Null;
        let queue = super::super::handlers::HandlerQueuePermit::try_acquire("demo/read", &args)
            .expect("queue reservation");
        let (reply, response) = tokio::sync::oneshot::channel();
        (
            IpcRequest {
                method: IpcMethod::Call {
                    queue_deadline: std::time::Instant::now(),
                    ticket: std::sync::Arc::new(super::super::handlers::HandlerCallTicket::new(
                        queue,
                        "demo/read".to_owned(),
                        args,
                    )),
                },
                reply,
                origin: 0,
                started_execution_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            },
            response,
        )
    }

    #[test]
    #[serial]
    fn handler_calls_keep_fifo_around_the_legacy_pending_slot() {
        let _guard = GlobalStateGuard;
        set_in_alternate_mode(false);
        set_r_at_prompt(true);
        let (first, first_reply) = expired_call();
        handle_request(first);
        let (reply, _response) = tokio::sync::oneshot::channel();
        handle_request(IpcRequest {
            method: IpcMethod::UserInput {
                code: "1".to_owned(),
            },
            reply,
            origin: 0,
            started_execution_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        });
        let (last, last_reply) = expired_call();
        handle_request(last);
        assert!(take_pending_ipc_operation().is_none());
        process_interactive_call();
        assert!(matches!(
            first_reply.blocking_recv().unwrap(),
            IpcResponse::Error {
                code: protocol::HANDLER_QUEUE_TIMEOUT,
                ..
            }
        ));
        // A later handler must wait for the accepted input operation.
        process_interactive_call();
        assert_eq!(interactive_calls().lock_recover().requests.len(), 1);
        assert!(take_pending_ipc_operation().is_some());
        process_interactive_call();
        assert!(matches!(
            last_reply.blocking_recv().unwrap(),
            IpcResponse::Error {
                code: protocol::HANDLER_QUEUE_TIMEOUT,
                ..
            }
        ));
    }

    #[test]
    #[serial]
    fn an_idle_callback_processes_only_one_handler_call() {
        let _guard = GlobalStateGuard;
        set_in_alternate_mode(false);
        set_r_at_prompt(true);
        let (first, _first_reply) = expired_call();
        let (second, _second_reply) = expired_call();
        handle_request(first);
        handle_request(second);
        process_interactive_call();
        assert_eq!(interactive_calls().lock_recover().requests.len(), 1);
    }

    /// Tests for the IN_ALTERNATE_MODE flag and handle_request rejection.
    ///
    /// Serialized with `#[serial]` because all tests that touch the global
    /// `IN_ALTERNATE_MODE` / `R_IS_AT_PROMPT` atomics must not run concurrently.
    #[test]
    #[serial]
    fn test_alternate_mode_flag_and_request_rejection() {
        // Reset global state and ensure cleanup on panic via Drop guard
        set_in_alternate_mode(false);
        set_r_at_prompt(false);
        let _guard = GlobalStateGuard;

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
                origin: 0,
                started_execution_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
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
                method: IpcMethod::SilentEvaluate {
                    code: "1+1".to_string(),
                    timeout: std::time::Duration::from_secs(1),
                },
                reply: reply_tx,
                origin: 0,
                started_execution_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            };
            handle_request(request);
            match reply_rx.blocking_recv().unwrap() {
                IpcResponse::Error { code, .. } => assert_eq!(code, R_NOT_AT_PROMPT),
                _ => panic!("Expected R_NOT_AT_PROMPT error for evaluate"),
            }
        }

        // Cleanup handled by GlobalStateGuard drop
    }

    /// Tests that `handle_request` returns arf-only session info (not an error)
    /// in various states: alternate mode, R busy, pending operation.
    // Protects the process-global `IN_ALTERNATE_MODE` / `R_IS_AT_PROMPT` atomics
    // and the `PENDING_IPC_OPERATION` static.
    #[test]
    #[serial]
    fn test_session_returns_arf_only_in_various_states() {
        set_in_alternate_mode(false);
        set_r_at_prompt(false);
        let _guard = GlobalStateGuard;

        // Helper: send a Session request and get the result
        fn send_session() -> protocol::SessionResult {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let request = IpcRequest {
                method: IpcMethod::Session,
                reply: reply_tx,
                origin: 0,
                started_execution_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            };
            handle_request(request);
            match reply_rx.blocking_recv().unwrap() {
                IpcResponse::Session(result) => *result,
                _ => panic!("Expected Session response"),
            }
        }

        // Case 1: alternate mode — should return arf-only with alternate mode reason
        set_in_alternate_mode(true);
        set_r_at_prompt(true);
        {
            let result = send_session();
            assert!(result.r.is_none());
            let reason = result.r_unavailable_reason.unwrap();
            assert!(
                reason.contains("alternate mode"),
                "Expected alternate mode reason, got: {reason}"
            );
        }

        // Case 2: R busy (not at prompt) — should return arf-only
        set_in_alternate_mode(false);
        set_r_at_prompt(false);
        {
            let result = send_session();
            assert!(result.r.is_none());
            assert!(result.r_unavailable_reason.is_some());
        }

        // Case 3: pending operation — should return arf-only
        set_r_at_prompt(true);
        {
            // Insert a dummy pending operation
            let (dummy_tx, _dummy_rx) = tokio::sync::oneshot::channel();
            *pending_ipc_operation().lock_recover() = Some(PendingIpcOperation {
                kind: PendingIpcKind::SilentEvaluate { reply: dummy_tx },
                code: "dummy".to_string(),
                origin: 0,
                started_execution_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            });

            let result = send_session();
            assert!(result.r.is_none());
            let reason = result.r_unavailable_reason.unwrap();
            assert!(
                reason.contains("pending"),
                "Expected pending reason, got: {reason}"
            );

            // Clean up dummy pending operation
            let _ = take_pending_ipc_operation();
        }
    }
}
