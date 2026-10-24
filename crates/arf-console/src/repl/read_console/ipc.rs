//! IPC operations that reach the console: code injection before the editor
//! starts reading, and operations that interrupt a running editor.

use super::prompt_kind::PromptKind;
use crate::history::HistoryRuntime;
use crate::ipc::{
    PendingIpcKind, accept_user_input, reject_operation_user_typing, run_silent_eval_from,
    setup_visible_eval,
};
use crate::repl::history::save_ipc_history;
use crate::repl::state::{PendingHistoryContext, PromptRuntimeConfig, ReplState};
use crossterm::{
    ExecutableCommand,
    style::Stylize,
    terminal::{self, ClearType},
};
use reedline::{HistorySessionId, Reedline};
use std::io::{self, Write};
use std::ops::ControlFlow;

struct ApprovedInteractiveIpcOperation {
    reply: tokio::sync::oneshot::Sender<crate::ipc::protocol::IpcResponse>,
    wrote_newline: bool,
}

struct AcceptedInteractiveIpcOperation {
    wrote_newline: bool,
    accepted_reply: Option<tokio::sync::oneshot::Sender<crate::ipc::protocol::IpcResponse>>,
}

/// Ask for approval for an operation that executes in the user's interactive
/// session, replying with the standard rejection when it is declined.
fn approve_interactive_ipc_operation(
    code: &str,
    reply: tokio::sync::oneshot::Sender<crate::ipc::protocol::IpcResponse>,
) -> Option<ApprovedInteractiveIpcOperation> {
    crate::ipc::set_r_at_prompt(false);
    let approval = crate::ipc::approve_user_input(code, &reply);
    if approval.approved {
        Some(ApprovedInteractiveIpcOperation {
            reply,
            wrote_newline: approval.wrote_newline,
        })
    } else {
        crate::ipc::set_r_at_prompt(true);
        crate::ipc::reject_user_input_not_approved(reply);
        None
    }
}

/// Approve a visible-evaluate or user-input operation and register its reply
/// channel for the evaluation that follows.
///
/// Returns whether the approval step already wrote a newline, or `None` when
/// the operation was rejected. Silent evaluation never reaches this function.
fn approve_and_accept(
    code: &str,
    kind: PendingIpcKind,
    origin: u64,
    started_execution_id: &std::sync::atomic::AtomicU64,
) -> Option<AcceptedInteractiveIpcOperation> {
    match kind {
        PendingIpcKind::VisibleEvaluate { reply, timeout } => {
            if reply.is_closed() {
                return None;
            }
            let ApprovedInteractiveIpcOperation {
                reply,
                wrote_newline,
            } = approve_interactive_ipc_operation(code, reply)?;
            if reply.is_closed() {
                crate::ipc::set_r_at_prompt(true);
                return None;
            }
            setup_visible_eval(reply, timeout, code, origin, started_execution_id);
            Some(AcceptedInteractiveIpcOperation {
                wrote_newline,
                accepted_reply: None,
            })
        }
        PendingIpcKind::UserInput { reply } => {
            if reply.is_closed() {
                return None;
            }
            let ApprovedInteractiveIpcOperation {
                reply,
                wrote_newline,
            } = approve_interactive_ipc_operation(code, reply)?;
            if reply.is_closed() {
                crate::ipc::set_r_at_prompt(true);
                return None;
            }
            Some(AcceptedInteractiveIpcOperation {
                wrote_newline,
                accepted_reply: Some(reply),
            })
        }
        PendingIpcKind::SilentEvaluate { .. } => {
            unreachable!("silent evaluation is handled before interactive acceptance")
        }
    }
}

/// The parts of the session that injecting IPC code into R touches.
struct IpcCodeInjection<'a> {
    editor: &'a mut Reedline,
    history: &'a HistoryRuntime,
    history_session_id: Option<HistorySessionId>,
    pending_history_context: &'a mut PendingHistoryContext,
    prompt_config: &'a mut PromptRuntimeConfig,
}

impl IpcCodeInjection<'_> {
    /// Inject accepted IPC code into the REPL as if the user had entered it.
    ///
    /// Saves the code to history, records it as the pending command context,
    /// calls `show_prompt` to display it, then starts timing and the spinner and
    /// marks R as busy. The caller returns `code` to R afterwards.
    fn inject(self, code: &str, show_prompt: impl FnOnce()) {
        let history_id = save_ipc_history(
            self.editor.history_mut(),
            self.history.store(),
            code,
            self.history_session_id,
        );
        if !code.trim().is_empty() {
            *self.pending_history_context = PendingHistoryContext::Command {
                store: self.history.store(),
                history_id,
            };
        }

        show_prompt();

        if !code.is_empty() {
            self.prompt_config.set_command_start();
            self.prompt_config.start_spinner();
        }

        crate::ipc::set_r_at_prompt(false);
    }
}

/// Handle a pending IPC operation before the editor starts reading input.
///
/// At this point reedline hasn't started, so there's no editor buffer to
/// conflict with — we can always accept. Returns the code to hand to R when an
/// operation was injected; otherwise the caller proceeds to read from the user.
pub(super) fn dispatch_pre_prompt_ipc(
    state: &mut ReplState,
    prompt_kind: PromptKind,
) -> Option<String> {
    if !prompt_kind.is_command() || state.prompt_config.is_shell_enabled() {
        return None;
    }
    let op = crate::ipc::take_pending_ipc_operation()?;

    if op.kind.reply_is_closed() {
        return None;
    }

    if let PendingIpcKind::SilentEvaluate { reply } = op.kind {
        // Run silent evaluate directly — no buffer conflict possible.
        // Unlike visible eval / user_input, silent eval does not return
        // code to R. Keep this background operation out of the user's REPL
        // command history, then fall through to the editor loop.
        let result = run_silent_eval_from(&op.code, op.origin, &op.started_execution_id);
        let _ = reply.send(crate::ipc::protocol::IpcResponse::Evaluate(result));
        return None;
    }

    let accepted = approve_and_accept(&op.code, op.kind, op.origin, &op.started_execution_id)?;
    IpcCodeInjection {
        editor: &mut state.line_editor,
        history: &state.r_history,
        history_session_id: state.history_session_id,
        pending_history_context: &mut state.pending_history_context,
        prompt_config: &mut state.prompt_config,
    }
    .inject(&op.code, || {
        let prompt_str = "agent> ";
        println!("{}{}", prompt_str.dark_cyan(), op.code);
    });
    if let Some(reply) = accepted.accepted_reply {
        accept_user_input(reply, &op.code, op.origin, &op.started_execution_id);
    }
    Some(op.code)
}

/// Clear the current terminal line and move the cursor to column 0.
fn clear_current_line(out: &mut io::Stdout) {
    let _ = out.execute(crossterm::cursor::MoveToColumn(0));
    let _ = out.execute(terminal::Clear(ClearType::CurrentLine));
}

/// Handle the break signal an IPC operation raises while the editor is
/// reading.
///
/// Checks the editor buffer for mutual exclusion with console input. Breaks
/// with the code to hand to R when an operation was injected; otherwise the
/// caller keeps waiting for input.
pub(super) fn handle_external_break(
    state: &mut ReplState,
    history_handle: &HistoryRuntime,
    is_shell_mode: bool,
    buffer: &str,
) -> ControlFlow<Option<String>> {
    // No pending operation (spurious signal), continue waiting
    let Some(op) = crate::ipc::take_pending_ipc_operation() else {
        return ControlFlow::Continue(());
    };

    if op.kind.reply_is_closed() {
        return ControlFlow::Continue(());
    }

    // If the user has typed something, reject the IPC operation.
    // We use trim() because reedline may include trailing whitespace
    // in the buffer; whitespace-only input is treated as empty.
    if !buffer.trim().is_empty() {
        reject_operation_user_typing(op, buffer);
        return ControlFlow::Continue(());
    }

    // Silent evaluate: run in-place and return to reedline
    if let PendingIpcKind::SilentEvaluate { reply } = op.kind {
        // Show a visual indicator and run without adding a command-history
        // entry; silent background queries should not affect REPL recall.
        {
            let mut out = io::stdout();
            clear_current_line(&mut out);
            print!("{}", "[evaluating...]".dark_cyan());
            let _ = out.flush();
        }

        let result = run_silent_eval_from(&op.code, op.origin, &op.started_execution_id);
        let _ = reply.send(crate::ipc::protocol::IpcResponse::Evaluate(result));

        // Clear the indicator — reedline will repaint the prompt
        clear_current_line(&mut io::stdout());

        return ControlFlow::Continue(());
    }

    // Visible evaluate / user input: accept, inject code into REPL.
    // Preserve whether approval already emitted its CRLF.
    let Some(accepted) = approve_and_accept(&op.code, op.kind, op.origin, &op.started_execution_id)
    else {
        return ControlFlow::Continue(());
    };

    let editor = if is_shell_mode {
        &mut state.shell_line_editor
    } else {
        &mut state.line_editor
    };
    IpcCodeInjection {
        editor,
        history: history_handle,
        history_session_id: state.history_session_id,
        pending_history_context: &mut state.pending_history_context,
        prompt_config: &mut state.prompt_config,
    }
    .inject(&op.code, || {
        // Clear the current prompt line and show agent prefix
        clear_current_line(&mut io::stdout());
        println!("{}{}", "agent> ".dark_cyan(), op.code);

        // ExternalBreak leaves reedline's previous prompt position suspended.
        // Its saved row range includes the line after a single-line prompt, so
        // leave the cursor one row beyond that range. Otherwise, when R produces
        // no output, the next repaint reuses the old prompt origin and clears the
        // echoed agent line.
        if !accepted.wrote_newline {
            println!();
        }
    });
    if let Some(reply) = accepted.accepted_reply {
        accept_user_input(reply, &op.code, op.origin, &op.started_execution_id);
    }
    ControlFlow::Break(Some(op.code))
}
