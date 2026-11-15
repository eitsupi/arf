//! Shared execution primitives for interactive and headless IPC frontends.

use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

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

pub(crate) struct ExecutionDeadlineNotification {
    pub execution_id: super::events::ExecutionId,
    pub deadline: Instant,
}

static EXECUTION_DEADLINE_NOTIFIER: OnceLock<
    Mutex<Option<tokio::sync::mpsc::UnboundedSender<ExecutionDeadlineNotification>>>,
> = OnceLock::new();

fn execution_deadline_notifier()
-> &'static Mutex<Option<tokio::sync::mpsc::UnboundedSender<ExecutionDeadlineNotification>>> {
    EXECUTION_DEADLINE_NOTIFIER.get_or_init(|| Mutex::new(None))
}

pub(crate) fn install_execution_deadline_notifier(
    sender: tokio::sync::mpsc::UnboundedSender<ExecutionDeadlineNotification>,
) {
    *execution_deadline_notifier()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
}

pub(crate) fn clear_execution_deadline_notifier() {
    *execution_deadline_notifier()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

fn notify_execution_deadline(notification: ExecutionDeadlineNotification) {
    let sender = execution_deadline_notifier()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(sender) = sender {
        let _ = sender.send(notification);
    }
}

pub(crate) fn request_execution_deadline(execution_id: u64) -> bool {
    if arf_libr::request_ipc_user_code_deadline(execution_id) {
        super::state::fire_break_signal();
        true
    } else {
        arf_libr::request_ipc_deadline(execution_id)
    }
}

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
    super::flush_console_output();
    if visible && kind == "user_code" {
        arf_harp::conditions::reset();
    }
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
    execution_timeout: Option<std::time::Duration>,
) -> super::events::ExecutionId {
    let id = begin_ipc_execution(
        ExecutionOrigin::Connection(origin),
        "handler",
        false,
        started_execution_id,
        false,
    );
    arf_libr::begin_handler_ipc_execution(id.0, execution_timeout.is_some());
    if let Some(timeout) = execution_timeout {
        notify_execution_deadline(ExecutionDeadlineNotification {
            execution_id: id,
            deadline: Instant::now()
                .checked_add(timeout)
                .unwrap_or_else(Instant::now),
        });
    }
    id
}

pub(crate) fn clear_execution() {
    super::flush_console_output();
    CURRENT_EXECUTION.take();
}

pub(crate) fn current_execution_id() -> Option<super::events::ExecutionId> {
    CURRENT_EXECUTION.get().map(|context| context.id)
}

/// Metadata used by the console observer; handler output is deliberately out
/// of scope for the public console output stream.
pub(super) fn console_output_context()
-> Option<(super::events::ExecutionId, serde_json::Value, bool)> {
    CURRENT_EXECUTION.with(|current| {
        let context = current.get()?;
        (context.kind == "user_code").then(|| {
            (
                context.id,
                serde_json::to_value(context.origin).expect("execution origin serializes"),
                context.visible,
            )
        })
    })
}

/// Final status and interrupt metadata retrieved before execution state is cleared.
pub(crate) struct ExecutionCompletion {
    pub status: ExecutionStatus,
    pub interrupt_reason: Option<String>,
}

impl ExecutionCompletion {
    pub fn apply(self, result: &mut EvaluateResult) {
        result.status = self.status;
        result.interrupt_reason = self.interrupt_reason;
        if self.status == ExecutionStatus::Interrupted {
            result.error = Some("Evaluation interrupted".to_string());
        }
    }
}

/// Publish completion on the R thread after evaluation notifications were queued.
pub(crate) fn finish_execution(status: ExecutionStatus) -> ExecutionCompletion {
    super::flush_console_output();
    let Some(context) = CURRENT_EXECUTION.take() else {
        return ExecutionCompletion {
            status: ExecutionStatus::Unknown,
            interrupt_reason: None,
        };
    };
    arf_libr::begin_ipc_execution_finishing();
    let (interrupted, deadline_requested) = arf_libr::finish_ipc_execution_outcome();
    let observed = if context.visible {
        arf_harp::conditions::outcome()
    } else {
        None
    };
    let status = if let Some(condition) = observed {
        match condition {
            arf_harp::conditions::UnhandledCondition::Error => ExecutionStatus::Error,
            arf_harp::conditions::UnhandledCondition::Interrupt => ExecutionStatus::Interrupted,
        }
    } else if interrupted || status == ExecutionStatus::Interrupted {
        ExecutionStatus::Interrupted
    } else {
        status
    };
    let completion = ExecutionCompletion {
        status,
        interrupt_reason: (deadline_requested && status == ExecutionStatus::Interrupted)
            .then(|| "execution_timeout".to_string()),
    };
    if !context.emit_completion {
        return completion;
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
    completion
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

fn start_eval_deadline(id: super::events::ExecutionId, timeout: Option<std::time::Duration>) {
    if let Some(timeout) = timeout
        && arf_libr::enable_ipc_execution_deadline()
    {
        notify_execution_deadline(ExecutionDeadlineNotification {
            execution_id: id,
            deadline: Instant::now()
                .checked_add(timeout)
                .unwrap_or_else(Instant::now),
        });
    }
}

/// Run a silent evaluation while keeping the shared R prompt state accurate.
///
/// History is frontend-specific: headless records external code, while the
/// interactive REPL leaves silent background evaluations out of command
/// recall.
pub(crate) fn run_silent_eval_from(
    code: &str,
    origin_id: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
    execution_timeout: Option<std::time::Duration>,
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
    start_eval_deadline(execution_id, execution_timeout);
    arf_libr::enable_ipc_silent_boundaries();
    r_is_at_prompt().store(false, Ordering::Release);
    // SAFETY: evaluate_with_capture enters R only through the protected harp
    // evaluation helper; this guard remains outside its top-level callbacks.
    let _interrupt_guard = unsafe { arf_libr::ConsoleEvaluationInterruptGuard::new() };
    let captured = {
        // Silent evaluation has no console to answer input requests.
        let _input_block = arf_libr::ConsoleInputBlockGuard::new();
        capture::evaluate_with_capture(code)
    };
    let mut result = captured.result;
    r_is_at_prompt().store(true, Ordering::Release);
    result.execution_id = Some(execution_id);
    let status = if captured.interrupted {
        arf_libr::mark_ipc_interrupt_observed();
        ExecutionStatus::Interrupted
    } else {
        result.status
    };
    finish_execution(status).apply(&mut result);
    result
}

/// Begin visible output capture and mark R busy before code is supplied.
pub(super) fn begin_visible_eval(
    code: &str,
    origin: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
    execution_timeout: Option<std::time::Duration>,
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
    start_eval_deadline(id, execution_timeout);
    r_is_at_prompt().store(false, Ordering::Release);
    arf_libr::start_ipc_capture(true);
    id
}

/// Finish visible capture and construct the common IPC result.
pub(super) fn finish_visible_eval() -> EvaluateResult {
    let (stdout, stderr) = arf_libr::finish_ipc_capture();
    let execution_id = current_execution_id();
    EvaluateResult {
        interrupt_reason: None,
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
    use super::has_user_code;

    #[test]
    fn blank_and_comment_only_input_does_not_emit_completion() {
        assert!(!has_user_code(" \t\n"));
        assert!(!has_user_code("# only a comment\n  # another comment\n"));
        assert!(has_user_code("# leading comment\nx <- 1"));
    }
}
