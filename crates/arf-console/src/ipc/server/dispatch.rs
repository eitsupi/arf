//! JSON-RPC method dispatch: parameter parsing, forwarding to the main thread,
//! and conversion of the main-thread reply into a JSON-RPC response.

use crate::ipc::execution::DEFAULT_EVAL_TIMEOUT;
use crate::ipc::handlers::{
    HandlerApiState, HandlerCallTicket, HandlerQueuePermit, handler_status,
};
use crate::ipc::history_query::{HistoryQueryError, query_history};
use crate::ipc::protocol::{
    DEFAULT_HANDLER_QUEUE_TIMEOUT, EvaluateParams, HANDLER_NOT_FOUND, HANDLER_QUEUE_FULL,
    HANDLER_QUEUE_TIMEOUT, HandlerCallParams, HistoryParams, INTERNAL_ERROR, INVALID_PARAMS,
    IpcMethod, IpcRequest, IpcResponse, JsonRpcRequest, JsonRpcResponse, MAX_HANDLER_QUEUE_TIMEOUT,
    METHOD_NOT_FOUND, R_NOT_AT_PROMPT, ShutdownResult, UserInputParams,
};
use crate::ipc::session_result::collect_session_result;
use crate::ipc::state::{has_headless_shutdown_handler, is_in_alternate_mode};
use crate::ipc::validation::{validate_code, validated_method};
use std::sync::mpsc;

/// Session info collection is lightweight; use a short timeout.
const SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Build an arf-only session response, falling back to INTERNAL_ERROR if
/// serialization fails (should never happen, but avoids panics in recovery paths).
fn session_fallback_response(id: Option<serde_json::Value>, reason: &str) -> JsonRpcResponse {
    match serde_json::to_value(collect_session_result(false, reason)) {
        Ok(val) => JsonRpcResponse::success(id, val),
        Err(e) => JsonRpcResponse::error(id, INTERNAL_ERROR, format!("Session info error: {e}")),
    }
}

/// Response for a request that could not reach, or get an answer from, the
/// main thread. `session` degrades to arf-only info; every other method
/// reports an internal error.
fn unavailable_response(
    id: Option<serde_json::Value>,
    is_session: bool,
    session_reason: &str,
    error_message: &str,
) -> JsonRpcResponse {
    if is_session {
        return session_fallback_response(id, session_reason);
    }
    JsonRpcResponse::error(id, INTERNAL_ERROR, error_message.to_string())
}

/// Reject immediately if in alternate mode (shell, history/help browser).
/// These modes block the main thread, so requests would hang in the mpsc
/// queue until the request timeout expires.
///
/// Exceptions: `session` and `history` are handled entirely on the server
/// thread (no main-thread dispatch needed), so they work in alternate mode.
fn alternate_mode_rejection(
    id: Option<serde_json::Value>,
    is_session: bool,
    is_history: bool,
) -> Option<JsonRpcResponse> {
    if !is_in_alternate_mode() {
        return None;
    }
    if is_session {
        return Some(session_fallback_response(
            id,
            "R is in alternate mode (shell, history browser, or help browser)",
        ));
    }
    if !is_history {
        return Some(JsonRpcResponse::error(
            id,
            R_NOT_AT_PROMPT,
            "R is not at the command prompt".to_string(),
        ));
    }
    None
}

/// Deserialize request params, or build the INVALID_PARAMS response.
pub(super) fn parse_params<T: serde::de::DeserializeOwned>(
    id: &Option<serde_json::Value>,
    params: serde_json::Value,
) -> Result<T, Box<JsonRpcResponse>> {
    serde_json::from_value(params).map_err(|e| {
        Box::new(JsonRpcResponse::error(
            id.clone(),
            INVALID_PARAMS,
            format!("Invalid params: {e}"),
        ))
    })
}

/// Shutdown is handled directly on the server thread — no need to send to the
/// main thread. Only available in headless mode.
fn handle_shutdown(id: Option<serde_json::Value>) -> JsonRpcResponse {
    if has_headless_shutdown_handler() {
        JsonRpcResponse::success(
            id,
            serde_json::to_value(ShutdownResult { accepted: true }).unwrap(),
        )
    } else {
        JsonRpcResponse::error(
            id,
            METHOD_NOT_FOUND,
            "shutdown is only available in headless mode".to_string(),
        )
    }
}

/// History is handled directly on the server thread — it only reads the
/// SQLite database and does not touch R state.
fn handle_history(id: Option<serde_json::Value>, params: serde_json::Value) -> JsonRpcResponse {
    // Treat missing/null params as empty object so callers can
    // rely on defaults (all fields have #[serde(default)]).
    let raw_params = if params.is_null() {
        serde_json::Value::Object(Default::default())
    } else {
        params
    };
    let params: HistoryParams = match parse_params(&id, raw_params) {
        Ok(p) => p,
        Err(response) => return *response,
    };
    match query_history(&params) {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                INTERNAL_ERROR,
                format!("Failed to serialize history result: {e}"),
            ),
        },
        Err(HistoryQueryError::InvalidParams(message)) => {
            JsonRpcResponse::error(id, INVALID_PARAMS, message)
        }
        Err(HistoryQueryError::Internal(message)) => {
            JsonRpcResponse::error(id, INTERNAL_ERROR, message)
        }
    }
}

/// Map a JSON-RPC method to the request forwarded to the main thread.
///
/// `Err` carries the final response for methods that are answered on the
/// server thread or rejected here (bad params, unknown method).
fn parse_method(
    id: &Option<serde_json::Value>,
    method: &str,
    params: serde_json::Value,
) -> Result<IpcMethod, Box<JsonRpcResponse>> {
    match method {
        "evaluate" => {
            let params: EvaluateParams = parse_params(id, params)?;
            validated_method(id, params.code, params.visible, params.timeout_ms)
        }
        "shutdown" => Err(Box::new(handle_shutdown(id.clone()))),
        "user_input" => {
            let params: UserInputParams = parse_params(id, params)?;
            validate_code(id, &params.code)?;
            Ok(IpcMethod::UserInput { code: params.code })
        }
        "session" => Ok(IpcMethod::Session),
        "call" => parse_handler_call(id, params),
        "history" => Err(Box::new(handle_history(id.clone(), params))),
        _ => Err(Box::new(JsonRpcResponse::error(
            id.clone(),
            METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        ))),
    }
}

fn parse_handler_call(
    id: &Option<serde_json::Value>,
    params: serde_json::Value,
) -> Result<IpcMethod, Box<JsonRpcResponse>> {
    let params: HandlerCallParams = parse_params(id, params)?;
    let timeout = params.queue_timeout_ms.map_or(
        DEFAULT_HANDLER_QUEUE_TIMEOUT,
        std::time::Duration::from_millis,
    );
    if timeout.is_zero() || timeout > MAX_HANDLER_QUEUE_TIMEOUT {
        return Err(Box::new(JsonRpcResponse::error(
            id.clone(),
            INVALID_PARAMS,
            format!(
                "queue_timeout_ms must be between 1 and {}",
                MAX_HANDLER_QUEUE_TIMEOUT.as_millis()
            ),
        )));
    }
    if handler_status().state != HandlerApiState::Locked {
        return Err(Box::new(JsonRpcResponse::error(
            id.clone(),
            HANDLER_NOT_FOUND,
            format!("Handler API is not available: {:?}", handler_status().state),
        )));
    }
    let queue =
        HandlerQueuePermit::try_acquire(&params.handler, &params.args).ok_or_else(|| {
            Box::new(JsonRpcResponse::error(
                id.clone(),
                HANDLER_QUEUE_FULL,
                "Handler call queue is full".to_string(),
            ))
        })?;
    let queued_at = std::time::Instant::now();
    let queue_deadline = queued_at.checked_add(timeout).unwrap_or(queued_at);
    Ok(IpcMethod::Call {
        queue_deadline,
        ticket: std::sync::Arc::new(HandlerCallTicket::new(queue, params.handler, params.args)),
    })
}

/// Extract the reply timeout from a validated method.
fn timeout_for(method: &IpcMethod) -> std::time::Duration {
    match method {
        IpcMethod::SilentEvaluate { timeout, .. } | IpcMethod::VisibleEvaluate { timeout, .. } => {
            *timeout
        }
        IpcMethod::Session => SESSION_TIMEOUT,
        _ => DEFAULT_EVAL_TIMEOUT,
    }
}

/// Convert the main thread's reply into a JSON-RPC response.
fn map_ipc_response(id: Option<serde_json::Value>, response: IpcResponse) -> JsonRpcResponse {
    match response {
        IpcResponse::Evaluate(result) => {
            JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
        }
        IpcResponse::Call(result) => JsonRpcResponse::success(id, result),
        IpcResponse::UserInput(result) => {
            JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
        }
        IpcResponse::Session(result) => {
            JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
        }
        IpcResponse::Error {
            code,
            message,
            data,
        } => {
            let mut resp = JsonRpcResponse::error(id, code, message);
            if let Some(ref mut err) = resp.error {
                err.data = data;
            }
            resp
        }
    }
}

/// Keep an execution identifiable even when its reply channel disappears or
/// the client's reply deadline expires after R has started running it.
fn with_execution_id(
    mut response: JsonRpcResponse,
    started: &std::sync::atomic::AtomicU64,
) -> JsonRpcResponse {
    let execution_id = started.load(std::sync::atomic::Ordering::Acquire);
    if execution_id != 0
        && let Some(error) = response.error.as_mut()
        && let Some(data) = error
            .data
            .get_or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
    {
        data.insert("execution_id".to_string(), serde_json::json!(execution_id));
    }
    response
}

#[cfg(test)]
pub(super) async fn dispatch_request(
    request: JsonRpcRequest,
    tx: &mpsc::Sender<IpcRequest>,
) -> JsonRpcResponse {
    let origin = crate::ipc::events::allocate_connection_id().value();
    dispatch_request_for_connection(request, tx, origin).await
}

pub(super) async fn dispatch_request_for_connection(
    request: JsonRpcRequest,
    tx: &mpsc::Sender<IpcRequest>,
    origin: u64,
) -> JsonRpcResponse {
    let started_execution_id = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let id = request.id.clone();
    let is_session = request.method == "session";
    let is_history = request.method == "history";
    let is_call = request.method == "call";

    if !is_call && let Some(response) = alternate_mode_rejection(id.clone(), is_session, is_history)
    {
        return response;
    }

    let method = match parse_method(&id, request.method.as_str(), request.params) {
        Ok(method) => method,
        Err(response) => return *response,
    };

    let timeout = timeout_for(&method);

    if let IpcMethod::Call {
        ref queue_deadline,
        ref ticket,
        ..
    } = method
    {
        let remaining = queue_deadline.saturating_duration_since(std::time::Instant::now());
        // Connection teardown drops this future. Race cancellation against
        // the R thread's queued -> started transition, rather than relying
        // only on a reply-channel check immediately before that transition.
        struct CancelQueuedCall(std::sync::Arc<HandlerCallTicket>);
        impl Drop for CancelQueuedCall {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let ticket = std::sync::Arc::clone(ticket);
        let _cancel_queued = CancelQueuedCall(std::sync::Arc::clone(&ticket));
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
        if tx
            .send(IpcRequest {
                method,
                reply: reply_tx,
                origin,
                started_execution_id: std::sync::Arc::clone(&started_execution_id),
            })
            .is_err()
        {
            return JsonRpcResponse::error(
                id,
                INTERNAL_ERROR,
                "Main thread unavailable".to_string(),
            );
        }
        let response = match tokio::time::timeout(remaining, &mut reply_rx).await {
            Ok(Ok(response)) => map_ipc_response(id, response),
            Ok(Err(_)) => JsonRpcResponse::error(
                id,
                INTERNAL_ERROR,
                "Handler request was dropped".to_string(),
            ),
            Err(_) => {
                if ticket.cancel() {
                    JsonRpcResponse::error(
                        id,
                        HANDLER_QUEUE_TIMEOUT,
                        "Handler call expired before it started".to_string(),
                    )
                } else {
                    match reply_rx.await {
                        Ok(response) => map_ipc_response(id, response),
                        Err(_) => JsonRpcResponse::error(
                            id,
                            INTERNAL_ERROR,
                            "Handler request was dropped".to_string(),
                        ),
                    }
                }
            }
        };
        return with_execution_id(response, &started_execution_id);
    }

    // Send to main thread and await response
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let ipc_request = IpcRequest {
        method,
        reply: reply_tx,
        origin,
        started_execution_id: std::sync::Arc::clone(&started_execution_id),
    };

    if tx.send(ipc_request).is_err() {
        // Return arf-only info if main thread is unavailable
        return unavailable_response(
            id,
            is_session,
            "Main thread is unavailable",
            "Main thread unavailable",
        );
    }

    // Wait for response from main thread (with timeout)
    let response = match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(response)) => map_ipc_response(id, response),
        Ok(Err(_)) => unavailable_response(
            id,
            is_session,
            "Request handler dropped",
            "Request handler dropped",
        ),
        Err(_) => unavailable_response(
            id,
            is_session,
            "Timed out collecting R session information",
            "Request timed out",
        ),
    };
    with_execution_id(response, &started_execution_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::handlers::{HandlerApiState, HandlerApiStatus, set_handler_status_for_test};
    use crate::ipc::protocol::SessionResult;
    use crate::ipc::set_in_alternate_mode;

    /// Drop guard that resets global IPC state on scope exit (including panics).
    struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            set_in_alternate_mode(false);
        }
    }

    struct HandlerStatusGuard;

    impl Drop for HandlerStatusGuard {
        fn drop(&mut self) {
            set_handler_status_for_test(HandlerApiStatus::default());
        }
    }

    fn enable_test_handlers() -> HandlerStatusGuard {
        set_handler_status_for_test(HandlerApiStatus {
            state: HandlerApiState::Locked,
            handlers: vec!["test/read".to_string()],
        });
        HandlerStatusGuard
    }

    async fn timed_out_call(
        id: u64,
        args: serde_json::Value,
        timeout_ms: u64,
        tx: &mpsc::Sender<IpcRequest>,
    ) -> JsonRpcResponse {
        dispatch_request(
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(serde_json::json!(id)),
                method: "call".to_string(),
                params: serde_json::json!({
                    "handler": "test/read",
                    "args": args,
                    "queue_timeout_ms": timeout_ms,
                }),
            },
            tx,
        )
        .await
    }

    async fn abort_queued_call(
        id: u64,
        args: serde_json::Value,
        tx: &mpsc::Sender<IpcRequest>,
        rx: &mpsc::Receiver<IpcRequest>,
    ) -> IpcRequest {
        let tx = tx.clone();
        let task = tokio::spawn(async move {
            dispatch_request(
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: Some(serde_json::json!(id)),
                    method: "call".to_string(),
                    params: serde_json::json!({
                        "handler": "test/read",
                        "args": args,
                        "queue_timeout_ms": 300_000,
                    }),
                },
                &tx,
            )
            .await
        });
        let queued = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match rx.try_recv() {
                    Ok(request) => break request,
                    Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        panic!("request channel disconnected")
                    }
                }
            }
        })
        .await
        .expect("call should reach the main-thread queue");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        queued
    }

    fn ticket_from(request: &IpcRequest) -> &std::sync::Arc<HandlerCallTicket> {
        match &request.method {
            IpcMethod::Call { ticket, .. } => ticket,
            _ => panic!("expected queued handler call"),
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn timed_out_calls_release_count_admission_while_requests_remain_queued() {
        let _status = enable_test_handlers();
        let (tx, rx) = mpsc::channel();

        // Keep canceled requests alive to simulate a busy R thread.
        for id in 0..63 {
            let response = timed_out_call(id, serde_json::Value::Null, 1, &tx).await;
            assert_eq!(response.error.unwrap().code, HANDLER_QUEUE_TIMEOUT);
        }
        let mut timed_out = Vec::new();
        for _ in 0..63 {
            timed_out.push(rx.try_recv().expect("timed-out request remains queued"));
        }
        let aborted = abort_queued_call(63, serde_json::Value::Null, &tx, &rx).await;
        assert!(ticket_from(&aborted).payload_is_empty());
        assert!(
            timed_out
                .iter()
                .all(|request| ticket_from(request).payload_is_empty())
        );

        let response = timed_out_call(64, serde_json::Value::Null, 1, &tx).await;
        assert_eq!(
            response.error.unwrap().code,
            HANDLER_QUEUE_TIMEOUT,
            "expired requests should release count permits before the R thread drains them"
        );

        let mut fresh_permits = Vec::new();
        for _ in 0..64 {
            fresh_permits.push(
                HandlerQueuePermit::try_acquire("h", &serde_json::Value::Null)
                    .expect("fresh reservations fit after cancellation"),
            );
        }
        assert!(HandlerQueuePermit::try_acquire("h", &serde_json::Value::Null).is_none());
        drop((aborted, timed_out));
        assert!(
            HandlerQueuePermit::try_acquire("h", &serde_json::Value::Null).is_none(),
            "dropping a stale ticket must not release a fresh ticket's reservation"
        );
        drop(rx);
        drop(fresh_permits);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn timed_out_calls_release_byte_admission_while_requests_remain_queued() {
        let _status = enable_test_handlers();
        let (tx, rx) = mpsc::channel();
        // The serialized argument plus handler name reaches the 8 MiB limit.
        let large_args = serde_json::Value::String("x".repeat(8 * 1024 * 1024 - 11));
        let response = timed_out_call(1, large_args, 1, &tx).await;
        assert_eq!(response.error.unwrap().code, HANDLER_QUEUE_TIMEOUT);
        let stale = rx.try_recv().expect("timed-out request remains queued");
        assert!(ticket_from(&stale).payload_is_empty());

        let response = timed_out_call(2, serde_json::Value::Null, 1, &tx).await;
        assert_eq!(
            response.error.unwrap().code,
            HANDLER_QUEUE_TIMEOUT,
            "expired request payload should release byte permits before the R thread drains it"
        );
        drop(stale);
        drop(rx);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn aborted_large_call_releases_byte_admission_and_payload() {
        let _status = enable_test_handlers();
        let (tx, rx) = mpsc::channel();
        let large_args = serde_json::Value::String("x".repeat(8 * 1024 * 1024 - 11));
        let stale = abort_queued_call(1, large_args, &tx, &rx).await;
        assert!(ticket_from(&stale).payload_is_empty());

        let response = timed_out_call(2, serde_json::Value::Null, 1, &tx).await;
        assert_eq!(response.error.unwrap().code, HANDLER_QUEUE_TIMEOUT);
        drop(stale);
        drop(rx);
    }

    /// Tests that dispatch_request rejects both evaluate and user_input
    /// in alternate mode.
    // Protects the process-global `IN_ALTERNATE_MODE` atomic.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_dispatch_rejects_in_alternate_mode() {
        set_in_alternate_mode(true);
        let _guard = Guard;

        // evaluate should be rejected
        let (tx, _rx) = mpsc::channel();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "evaluate".to_string(),
            params: serde_json::json!({"code": "1+1"}),
            id: Some(serde_json::json!(1)),
        };
        let response = dispatch_request(request, &tx).await;
        assert_eq!(response.error.unwrap().code, R_NOT_AT_PROMPT);

        // user_input should also be rejected
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "user_input".to_string(),
            params: serde_json::json!({"code": "print('hello')"}),
            id: Some(serde_json::json!(2)),
        };
        let response = dispatch_request(request, &tx).await;
        assert_eq!(response.error.unwrap().code, R_NOT_AT_PROMPT);

        // Cleanup handled by Guard drop
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn nul_code_is_rejected_before_main_thread_dispatch() {
        set_in_alternate_mode(false);
        let _guard = Guard;
        let (tx, rx) = mpsc::channel();

        for (method, params) in [
            ("evaluate", serde_json::json!({"code": "function(x) {\0"})),
            (
                "evaluate",
                serde_json::json!({"code": "function(x) {\0", "visible": true}),
            ),
            ("user_input", serde_json::json!({"code": "function(x) {\0"})),
        ] {
            let response = dispatch_request(
                JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: method.to_string(),
                    params,
                    id: Some(serde_json::json!(1)),
                },
                &tx,
            )
            .await;
            assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
            assert!(rx.try_recv().is_err());
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn requested_timeout_expires_while_evaluate_is_queued_in_both_modes() {
        set_in_alternate_mode(false);
        let _guard = Guard;

        for visible in [false, true] {
            let (tx, rx) = mpsc::channel();
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "evaluate".to_string(),
                params: serde_json::json!({
                    "code": "1",
                    "visible": visible,
                    "timeout_ms": 25,
                }),
                id: Some(serde_json::json!(1)),
            };
            let dispatch_tx = tx.clone();
            let task = tokio::spawn(async move { dispatch_request(request, &dispatch_tx).await });
            let queued = tokio::task::spawn_blocking(move || {
                rx.recv().expect("evaluation should be queued")
            })
            .await
            .expect("receiver task should complete");

            match (&queued.method, visible) {
                (IpcMethod::SilentEvaluate { timeout, .. }, false)
                | (IpcMethod::VisibleEvaluate { timeout, .. }, true) => {
                    assert_eq!(*timeout, std::time::Duration::from_millis(25));
                }
                _ => panic!("evaluation visibility was not preserved in the queued method"),
            }

            let response = task.await.expect("dispatch task should complete");
            assert_eq!(response.error.unwrap().code, INTERNAL_ERROR);
            assert!(queued.reply.is_closed());
        }
    }

    /// Tests that `session` returns arf-only success (not an error) in alternate mode,
    /// with a context-appropriate `r_unavailable_reason`.
    // Protects the process-global `IN_ALTERNATE_MODE` atomic and `SESSION_META`
    // session-metadata cache.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_session_returns_arf_only_in_alternate_mode() {
        set_in_alternate_mode(true);
        let _guard = Guard;

        let (tx, _rx) = mpsc::channel();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "session".to_string(),
            params: serde_json::json!({}),
            id: Some(serde_json::json!(1)),
        };
        let response = dispatch_request(request, &tx).await;

        // Should be a success, not an error
        assert!(
            response.error.is_none(),
            "session should not return an error"
        );
        let result_value = response.result.expect("session should return a result");
        let result: SessionResult =
            serde_json::from_value(result_value).expect("should parse as SessionResult");

        // Should have arf info
        assert!(!result.arf_version.is_empty());
        assert!(result.pid > 0);

        // R info should be absent with an explanation
        assert!(
            result.r.is_none(),
            "R info should be null in alternate mode"
        );
        let reason = result
            .r_unavailable_reason
            .expect("should have r_unavailable_reason");
        assert!(
            reason.contains("alternate mode"),
            "reason should mention alternate mode, got: {reason}"
        );
        assert!(result.hint.is_some(), "should have a hint");
    }

    /// Tests that `session` returns arf-only info when the main thread channel
    /// is broken (tx.send fails).
    // Protects the process-global `IN_ALTERNATE_MODE` atomic and `SESSION_META`
    // session-metadata cache.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_session_fallback_on_channel_failure() {
        set_in_alternate_mode(false);
        let _guard = Guard;

        // Create a channel and immediately drop the receiver so send() fails
        let (tx, _rx) = mpsc::channel();
        drop(_rx);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "session".to_string(),
            params: serde_json::json!({}),
            id: Some(serde_json::json!(1)),
        };
        let response = dispatch_request(request, &tx).await;

        // Should be a success with arf-only info
        assert!(
            response.error.is_none(),
            "session should not return an error"
        );
        let result_value = response.result.expect("session should return a result");
        let result: SessionResult =
            serde_json::from_value(result_value).expect("should parse as SessionResult");

        assert!(result.r.is_none(), "R info should be null");
        assert!(
            result.r_unavailable_reason.is_some(),
            "should have r_unavailable_reason"
        );
    }
}
