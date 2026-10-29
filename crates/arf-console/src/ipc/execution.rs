//! Shared execution primitives for interactive and headless IPC frontends.

use std::sync::atomic::Ordering;

use serde::Serialize;
use serde_json::json;

use super::capture;
use super::protocol::{EvaluateResult, ExecutionStatus, IpcResponse, UserInputResult};
use super::state::r_is_at_prompt;

/// Origin recorded for one evaluated unit of user code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    static LAST_FINISHED_EXECUTION: std::cell::Cell<Option<(u64, ExecutionStatus)>> = const { std::cell::Cell::new(None) };
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
pub(crate) fn finish_execution(status: ExecutionStatus) -> ExecutionStatus {
    let Some(context) = CURRENT_EXECUTION.take() else {
        return ExecutionStatus::Unknown;
    };
    arf_libr::begin_ipc_execution_finishing();
    let status = if arf_libr::finish_ipc_execution() || status == ExecutionStatus::Interrupted {
        ExecutionStatus::Interrupted
    } else {
        status
    };
    LAST_FINISHED_EXECUTION.set(Some((context.id.0, status)));
    if !context.emit_completion {
        return status;
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
    status
}

pub(crate) fn finished_execution_status(
    execution_id: super::events::ExecutionId,
) -> ExecutionStatus {
    LAST_FINISHED_EXECUTION
        .get()
        .filter(|(id, _)| *id == execution_id.0)
        .map(|(_, status)| status)
        .unwrap_or(ExecutionStatus::Unknown)
}

/// Request an interrupt for the active IPC-originated user-code evaluation.
pub(crate) fn request_interrupt(execution_id: Option<u64>) -> super::protocol::InterruptResult {
    let (accepted, active_id) = arf_libr::request_ipc_interrupt(execution_id);
    if accepted {
        super::state::fire_break_signal();
    }
    super::protocol::InterruptResult {
        accepted,
        execution_id: accepted.then(|| {
            super::events::ExecutionId(active_id.expect("accepted interrupt has an execution ID"))
        }),
    }
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
    arf_libr::reset_command_error_state();
    let execution_id = begin_ipc_execution(
        ExecutionOrigin::Connection(origin_id),
        "user_code",
        false,
        started_execution_id,
        has_user_code(code),
    );
    arf_libr::begin_ipc_execution(execution_id.0, has_user_code(code));
    r_is_at_prompt().store(false, Ordering::Release);
    arf_libr::set_ipc_execution_phase(arf_libr::InterruptPhase::ProtectedEvaluation);
    // SAFETY: evaluate_with_capture enters R only through the protected harp
    // evaluation helper; this guard remains outside its top-level callbacks.
    let _interrupt_guard = unsafe { arf_libr::ConsoleEvaluationInterruptGuard::new() };
    let captured = capture::evaluate_with_capture(code);
    let mut result = captured.result;
    r_is_at_prompt().store(true, Ordering::Release);
    result.execution_id = Some(execution_id);
    let status = if captured.interrupted {
        arf_libr::mark_ipc_interrupt_observed();
        ExecutionStatus::Interrupted
    } else {
        result.status
    };
    result.status = finish_execution(status);
    if result.status == ExecutionStatus::Interrupted {
        result.error = Some("Evaluation interrupted".to_string());
    }
    result
}

/// Begin visible output capture and mark R busy before code is supplied.
pub(super) fn begin_visible_eval(
    code: &str,
    origin: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
) -> super::events::ExecutionId {
    arf_libr::reset_command_error_state();
    let id = begin_ipc_execution(
        ExecutionOrigin::Connection(origin),
        "user_code",
        true,
        started_execution_id,
        has_user_code(code),
    );
    arf_libr::begin_ipc_execution(id.0, has_user_code(code));
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
        status: ExecutionStatus::Unknown,
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
    arf_libr::reset_command_error_state();
    let execution_id = begin_ipc_execution(
        ExecutionOrigin::Connection(origin),
        "user_code",
        true,
        started_execution_id,
        has_user_code(code),
    );
    arf_libr::begin_ipc_execution(execution_id.0, has_user_code(code));
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
    use super::{
        ExecutionStatus, LAST_FINISHED_EXECUTION, finished_execution_status, has_user_code,
    };

    #[test]
    fn blank_and_comment_only_input_does_not_emit_completion() {
        assert!(!has_user_code(" \t\n"));
        assert!(!has_user_code("# only a comment\n  # another comment\n"));
        assert!(has_user_code("# leading comment\nx <- 1"));
    }

    #[test]
    fn finished_execution_status_is_unknown_without_a_matching_id() {
        LAST_FINISHED_EXECUTION.set(Some((41, ExecutionStatus::Error)));

        assert_eq!(
            finished_execution_status(super::super::events::ExecutionId(42)),
            ExecutionStatus::Unknown
        );
        assert_eq!(
            finished_execution_status(super::super::events::ExecutionId(41)),
            ExecutionStatus::Error
        );

        LAST_FINISHED_EXECUTION.set(None);
    }
}
