//! Shared execution primitives for interactive and headless IPC frontends.

use std::sync::atomic::Ordering;

use super::capture;
use super::protocol::{EvaluateResult, IpcResponse, UserInputResult};
use super::state::r_is_at_prompt;

pub(super) const DEFAULT_EVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Run a silent evaluation while keeping the shared R prompt state accurate.
///
/// History is frontend-specific: headless records external code, while the
/// interactive REPL leaves silent background evaluations out of command
/// recall.
pub(crate) fn run_silent_eval(code: &str) -> EvaluateResult {
    r_is_at_prompt().store(false, Ordering::Release);
    let result = capture::evaluate_with_capture(code);
    r_is_at_prompt().store(true, Ordering::Release);
    result
}

/// Begin visible output capture and mark R busy before code is supplied.
pub(super) fn begin_visible_eval() {
    r_is_at_prompt().store(false, Ordering::Release);
    arf_libr::start_ipc_capture(true);
}

/// Finish visible capture and construct the common IPC result.
pub(super) fn finish_visible_eval() -> EvaluateResult {
    let (stdout, stderr) = arf_libr::finish_ipc_capture();
    EvaluateResult {
        stdout,
        stderr,
        value: None,
        error: None,
    }
}

/// Reply that a `user_input` request has entered its execution queue.
pub(crate) fn accept_user_input(reply: tokio::sync::oneshot::Sender<IpcResponse>) {
    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
}
