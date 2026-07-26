use super::*;
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
            method: IpcMethod::Evaluate {
                code: "1+1".to_string(),
                visible: false,
                timeout_ms: None,
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
        *pending_ipc_operation()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(PendingIpcOperation {
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
