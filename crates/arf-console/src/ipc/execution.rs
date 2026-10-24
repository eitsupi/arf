//! Shared execution primitives for interactive and headless IPC frontends.

use std::sync::atomic::Ordering;

use serde::Serialize;
use serde_json::json;

use super::capture;
use super::protocol::{EvaluateResult, IpcResponse, UserInputResult};
use super::state::r_is_at_prompt;

/// Origin recorded for one evaluated unit of user code.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(untagged)]
pub(crate) enum ExecutionOrigin {
    Terminal(&'static str),
    Connection(u64),
}

#[derive(Debug, Clone, Copy)]
struct ExecutionContext {
    id: super::events::ExecutionId,
    origin: ExecutionOrigin,
    kind: &'static str,
    visible: bool,
    emit_completion: bool,
}

static NEXT_EXECUTION_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    /// The R thread executes one top-level command at a time. Keep its context
    /// installed across ReadConsole callbacks so notifications inherit the ID.
    static CURRENT_EXECUTION: std::cell::Cell<Option<ExecutionContext>> = const { std::cell::Cell::new(None) };
}

fn begin_execution_kind(
    origin: ExecutionOrigin,
    kind: &'static str,
    visible: bool,
    emit_completion: bool,
) -> super::events::ExecutionId {
    let id = super::events::ExecutionId(NEXT_EXECUTION_ID.fetch_add(1, Ordering::Relaxed));
    CURRENT_EXECUTION.set(Some(ExecutionContext {
        id,
        origin,
        kind,
        visible,
        emit_completion,
    }));
    id
}

fn begin_ipc_execution(
    origin: ExecutionOrigin,
    kind: &'static str,
    visible: bool,
    started_execution_id: &std::sync::atomic::AtomicU64,
    emit_completion: bool,
) -> super::events::ExecutionId {
    let id = begin_execution_kind(origin, kind, visible, emit_completion);
    started_execution_id.store(id.0, Ordering::Release);
    id
}

pub(crate) fn begin_handler_execution(
    origin: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
) -> super::events::ExecutionId {
    begin_ipc_execution(
        ExecutionOrigin::Connection(origin),
        "handler",
        false,
        started_execution_id,
        false,
    )
}

pub(crate) fn clear_execution() {
    CURRENT_EXECUTION.take();
}

pub(crate) fn current_execution_id() -> Option<super::events::ExecutionId> {
    CURRENT_EXECUTION.get().map(|context| context.id)
}

/// Publish one completion and clear the active context. This must run on the
/// R thread after all notifications produced by the evaluation were queued.
pub(crate) fn finish_execution(status: ExecutionStatus) {
    let Some(context) = CURRENT_EXECUTION.take() else {
        return;
    };
    if !context.emit_completion {
        return;
    }
    let topic = super::events::Topic::internal("arf.execution_completed")
        .expect("the built-in execution topic is valid");
    let data = json!({
        "id": context.id,
        "origin": context.origin,
        "kind": context.kind,
        "visible": context.visible,
        "status": status,
    });
    if let Err(error) = super::events::publish_event(topic, data, Some(context.id)) {
        log::debug!("Could not publish execution completion: {error}");
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ExecutionStatus {
    Ok,
    Error,
    Unknown,
}

pub(super) const DEFAULT_EVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Run a silent evaluation while keeping the shared R prompt state accurate.
///
/// History is frontend-specific: headless records external code, while the
/// interactive REPL leaves silent background evaluations out of command
/// recall.
pub(crate) fn run_silent_eval_from(
    code: &str,
    origin_id: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
) -> EvaluateResult {
    let execution_id = begin_ipc_execution(
        ExecutionOrigin::Connection(origin_id),
        "user_code",
        false,
        started_execution_id,
        has_user_code(code),
    );
    r_is_at_prompt().store(false, Ordering::Release);
    let mut result = capture::evaluate_with_capture(code);
    r_is_at_prompt().store(true, Ordering::Release);
    result.execution_id = Some(execution_id);
    finish_execution(if result.error.is_some() {
        ExecutionStatus::Error
    } else {
        ExecutionStatus::Ok
    });
    result
}

/// Begin visible output capture and mark R busy before code is supplied.
pub(super) fn begin_visible_eval(
    code: &str,
    origin: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
) -> super::events::ExecutionId {
    let id = begin_ipc_execution(
        ExecutionOrigin::Connection(origin),
        "user_code",
        true,
        started_execution_id,
        has_user_code(code),
    );
    r_is_at_prompt().store(false, Ordering::Release);
    arf_libr::start_ipc_capture(true);
    id
}

/// Finish visible capture and construct the common IPC result.
pub(super) fn finish_visible_eval() -> EvaluateResult {
    let (stdout, stderr) = arf_libr::finish_ipc_capture();
    let execution_id = current_execution_id();
    EvaluateResult {
        stdout,
        stderr,
        value: None,
        error: None,
        execution_id,
    }
}

/// Reply that a `user_input` request has entered its execution queue.
pub(crate) fn accept_user_input(
    reply: tokio::sync::oneshot::Sender<IpcResponse>,
    code: &str,
    origin: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
) {
    let execution_id = begin_ipc_execution(
        ExecutionOrigin::Connection(origin),
        "user_code",
        true,
        started_execution_id,
        has_user_code(code),
    );
    let _ = reply.send(IpcResponse::UserInput(UserInputResult {
        accepted: true,
        execution_id: Some(execution_id),
    }));
}

/// Begin tracking a terminal submission until R returns to its top-level prompt.
pub(crate) fn begin_terminal_execution(code: &str) -> super::events::ExecutionId {
    begin_execution_kind(
        ExecutionOrigin::Terminal("terminal"),
        "user_code",
        true,
        has_user_code(code),
    )
}

/// Ignore blank lines and full-line R comments when deciding whether an
/// accepted input can produce user code. R handles the actual parsing.
fn has_user_code(code: &str) -> bool {
    code.lines().any(|line| {
        let line = line.trim_start();
        !line.is_empty() && !line.starts_with('#')
    })
}

#[cfg(test)]
mod tests {
    use super::has_user_code;

    #[test]
    fn blank_and_comment_only_input_does_not_emit_completion() {
        assert!(!has_user_code(" \t\n"));
        assert!(!has_user_code("# only a comment\n  # another comment\n"));
        assert!(has_user_code("# leading comment\nx <- 1"));
    }
}
