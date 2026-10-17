//! REPL-side handling of IPC requests.
//!
//! Requests that could conflict with console input are stored as a single
//! [`PendingIpcOperation`] and the REPL is interrupted so it can check the
//! editor buffer before accepting or rejecting the operation.

use super::execution;
use super::execution::DEFAULT_EVAL_TIMEOUT;
use super::protocol::{
    INPUT_ALREADY_PENDING, IpcMethod, IpcRequest, IpcResponse, R_BUSY, R_NOT_AT_PROMPT,
    USER_IS_TYPING,
};
use super::session_result::collect_session_result;
use super::state::{drain_requests, fire_break_signal, is_in_alternate_mode, r_is_at_prompt};
use crate::sync::LockRecover;
use std::sync::{Mutex, OnceLock, atomic::Ordering};

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
    /// Absolute deadline — best-effort aligned with the server-side timeout so that
    /// REPL-side cleanup should not outlive the server's oneshot wait under normal conditions.
    deadline: std::time::Instant,
    /// Original timeout duration, kept for diagnostic messages.
    timeout: std::time::Duration,
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
}

/// Check if a pending visible evaluate has completed.
///
/// A visible evaluate injects code into the REPL (like user_input) and waits
/// for the result. When R returns to the prompt, the evaluation is done and
/// we can collect captured output and send the reply.
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
        && std::time::Instant::now() > pending.deadline
    {
        let pending = guard.take().unwrap();
        // Best-effort capture cleanup (may race with active R evaluation)
        let result = execution::finish_visible_eval();
        reply_error(
            pending.reply,
            R_BUSY,
            format!(
                "Visible evaluate timed out after {}s (stdout: {} bytes, stderr: {} bytes)",
                pending.timeout.as_secs(),
                result.stdout.len(),
                result.stderr.len(),
            ),
        );
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
        let result = execution::finish_visible_eval();

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
    let IpcRequest { method, reply } = request;

    if reply.is_closed() {
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
            });
            fire_break_signal();
        }
        IpcMethod::VisibleEvaluate { code, timeout } => {
            if !r_is_at_prompt().load(Ordering::Acquire) {
                reply_error(reply, R_BUSY, "R is busy");
                return;
            }
            *pending_ipc_operation().lock_recover() = Some(PendingIpcOperation {
                kind: PendingIpcKind::VisibleEvaluate { reply, timeout },
                code,
            });
            fire_break_signal();
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
            });
            fire_break_signal();
        }
        IpcMethod::Session => unreachable!("Session handled above"),
    }
}

/// Take the pending IPC operation (if any).
///
/// Called from the REPL's ExternalBreak handler and fast-path to process
/// the stored operation after checking the editor buffer.
pub fn take_pending_ipc_operation() -> Option<PendingIpcOperation> {
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
    timeout: std::time::Duration,
) {
    execution::begin_visible_eval();

    // Store reply channel — will be consumed by check_visible_eval_completion.
    // The deadline is set from now + timeout, aligning with the server-side
    // oneshot timeout that started slightly earlier in dispatch_request.
    *pending_visible_eval().lock_recover() = Some(PendingVisibleEval {
        reply,
        deadline: std::time::Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(|| std::time::Instant::now() + DEFAULT_EVAL_TIMEOUT),
        timeout,
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
        }
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
