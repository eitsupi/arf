//! JSON-RPC method dispatch: parameter parsing, forwarding to the main thread,
//! and conversion of the main-thread reply into a JSON-RPC response.

use crate::ipc::execution::DEFAULT_EVAL_TIMEOUT;
use crate::ipc::history_query::{HistoryQueryError, query_history};
use crate::ipc::protocol::{
    EvaluateParams, HistoryParams, INTERNAL_ERROR, INVALID_PARAMS, IpcMethod, IpcRequest,
    IpcResponse, JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND, R_NOT_AT_PROMPT,
    ShutdownResult, UserInputParams,
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
        "history" => Err(Box::new(handle_history(id.clone(), params))),
        _ => Err(Box::new(JsonRpcResponse::error(
            id.clone(),
            METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        ))),
    }
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

pub(super) async fn dispatch_request(
    request: JsonRpcRequest,
    tx: &mpsc::Sender<IpcRequest>,
) -> JsonRpcResponse {
    let id = request.id.clone();
    let is_session = request.method == "session";
    let is_history = request.method == "history";

    if let Some(response) = alternate_mode_rejection(id.clone(), is_session, is_history) {
        return response;
    }

    let method = match parse_method(&id, request.method.as_str(), request.params) {
        Ok(method) => method,
        Err(response) => return *response,
    };

    let timeout = timeout_for(&method);

    // Send to main thread and await response
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let ipc_request = IpcRequest {
        method,
        reply: reply_tx,
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
    match tokio::time::timeout(timeout, reply_rx).await {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::SessionResult;
    use crate::ipc::set_in_alternate_mode;

    /// Drop guard that resets global IPC state on scope exit (including panics).
    struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            set_in_alternate_mode(false);
        }
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
