//! IPC request handling for headless mode.
//!
//! There is no interactive editor in headless mode, so requests are processed
//! directly on the R thread without the ExternalBreak/editor-buffer check.

use super::capture;
use super::history_query::set_history_store;
use super::policy;
use super::protocol::{IpcMethod, IpcRequest, IpcResponse, UserInputResult};
use super::session_result::collect_session_result;
use super::state::{drain_requests, r_is_at_prompt};
use crate::history::{HistoryExtraInfo, HistoryStore};
use std::sync::{OnceLock, atomic::Ordering};

/// History backend for headless mode. When set, evaluated commands are
/// persisted to the same SQLite history database used by the REPL.
static HEADLESS_HISTORY: OnceLock<HistoryStore> = OnceLock::new();

/// Set the history backend for headless mode.
///
/// Once set, `headless_handle_request` will persist evaluated commands
/// (both `evaluate` and `user_input`) to the SQLite history database.
pub fn set_headless_history(history: HistoryStore) {
    let ipc_history = history.clone();
    if HEADLESS_HISTORY.set(history).is_err() {
        log::warn!(
            "Headless history backend already initialized; ignoring duplicate set_headless_history call"
        );
    }
    set_history_store(ipc_history);
}

/// Save a command to the headless history database, if configured.
///
/// Errors are logged but never propagated — history saving must not
/// interfere with IPC response delivery.
fn save_to_headless_history(code: &str, exit_status: Option<i64>) {
    let Some(h) = HEADLESS_HISTORY.get() else {
        return;
    };
    let mut item = reedline::HistoryItem::from_command_line(code);
    item.start_timestamp = Some(chrono::Utc::now());
    item.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
    item.cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    item.exit_status = exit_status;
    item.session_id = HEADLESS_HISTORY.get().and_then(HistoryStore::session);
    if let Err(e) = h.save_known(item, HistoryExtraInfo::default()) {
        log::warn!("Failed to save headless history: {}", e);
    }
}

/// Poll and directly process IPC requests in headless mode.
///
/// Unlike `poll_ipc_requests` (used by the REPL), this function processes
/// requests immediately without the ExternalBreak/editor-buffer check,
/// since there is no interactive editor in headless mode.
///
/// Returns `true` if at least one request was processed.
pub fn headless_poll_and_process() -> bool {
    drain_requests(headless_handle_request)
}

/// Handle a single IPC request in headless mode.
///
/// Processes evaluate and user_input requests directly on the R thread.
/// No editor buffer check is needed since there is no interactive console.
fn headless_handle_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;

    match method {
        IpcMethod::Evaluate { code, visible, .. } => {
            if !visible && let Some(rejection) = policy::policy_reject(&code) {
                let _ = reply.send(rejection);
                return;
            }
            // Note: timeout_ms is intentionally ignored here. In headless mode,
            // evaluate_with_capture() runs synchronously on the main thread, so
            // R evaluation cannot be interrupted. The server-side oneshot timeout
            // (in dispatch_request) still applies as a backstop.
            //
            // When visible=true, captured output is also written to the
            // headless process's stdout/stderr for logging/monitoring.
            r_is_at_prompt().store(false, Ordering::Release);
            let result = capture::evaluate_with_capture(&code, visible);
            r_is_at_prompt().store(true, Ordering::Release);

            // Determine exit status before moving result into the reply.
            let has_error = result.error.is_some();
            let _ = reply.send(IpcResponse::Evaluate(result));

            // Save after reply so SQLite I/O doesn't delay the IPC response.
            if !code.trim().is_empty() {
                let exit_status = if has_error { 1 } else { 0 };
                save_to_headless_history(&code, Some(exit_status));
            }
        }
        IpcMethod::UserInput { code } => {
            // In headless mode, user_input evaluates the code directly.
            // Output goes to the default WriteConsoleEx handler (stdout/stderr).
            r_is_at_prompt().store(false, Ordering::Release);
            let eval_result = arf_harp::eval_string(&code);
            r_is_at_prompt().store(true, Ordering::Release);
            let exit_status;
            match eval_result {
                Ok(_) => {
                    exit_status = 0;
                    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
                }
                Err(e) => {
                    exit_status = 1;
                    log::warn!("Headless user_input evaluation error: {}", e);
                    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: false }));
                }
            }

            if !code.trim().is_empty() {
                save_to_headless_history(&code, Some(exit_status));
            }
        }
        IpcMethod::Session => {
            let result = collect_session_result(true, "");
            let _ = reply.send(IpcResponse::Session(Box::new(result)));
        }
    }
}
