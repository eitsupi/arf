//! Handling of the results returned by the line editor.

use super::lifecycle::record_formatter_failure;
use super::prompt_kind::PromptKind;
use crate::history::HistoryRuntime;
use crate::repl::history::finalize_history;
use crate::repl::meta_command::{
    MetaAction, MetaCommandContext, SessionInfoContext, dir_command_hint,
    handle_meta_command_result, process_meta_command,
};
use crate::repl::state::{PendingHistoryContext, ReplState};
use crate::repl::{arf_println, clear_input_lines, execute_shell_command, strip_reprex_output};
use crossterm::{
    ExecutableCommand,
    terminal::{self, ClearType},
};
use std::io;
use std::ops::ControlFlow;

/// Handle a line the user submitted.
///
/// Breaks with the value ReadConsole returns to R (`None` ends the session);
/// continues when the editor should prompt again.
pub(super) fn handle_line(
    state: &mut ReplState,
    history_handle: &HistoryRuntime,
    prompt_kind: PromptKind,
    is_shell_mode: bool,
    line: String,
) -> ControlFlow<Option<String>> {
    let save_outcome = history_handle.receipt_outcome();

    // For non-standard prompts (menus, etc.), pass input directly to R
    // without any processing (meta commands, shell mode, or reprex)
    if prompt_kind.is_other() {
        // Deliberately leave pending_history_context alone. This
        // input was requested by R during an evaluation that is
        // already in progress (readline(), menu(), browser()),
        // so the outer command still owns the result. Claiming
        // the context here would strand the outer entry without
        // an exit status, which matters most when that entry
        // came from IPC and cannot be recovered from reedline's
        // own last-command context.
        finalize_history(Some(history_handle), save_outcome, false);
        return ControlFlow::Break(Some(line));
    }

    // Check for meta commands first
    let mut meta_ctx = MetaCommandContext {
        prompt_config: &mut state.prompt_config,
        reprex: &mut state.reprex,
        r_history: &state.r_history,
        shell_history: &state.shell_history,
        r_source_status: &state.r_source_status,
        dir_stack: &mut state.dir_stack,
        history_session_id: state.history_session_id.map(i64::from),
        r_home: state.r_home.as_deref(),
    };
    if let Some(result) = process_meta_command(&line, &mut meta_ctx) {
        finalize_history(Some(history_handle), save_outcome, true);
        // Clear duration so the previous R command's time
        // does not persist in the prompt after a meta command.
        state.prompt_config.clear_command_duration();
        let ctx = SessionInfoContext {
            prompt_config: &state.prompt_config,
            reprex: &state.reprex,
            config_file_info: &state.config_file_info,
            history_location: &state.history_location,
            r_history: &state.r_history,
            shell_history: &state.shell_history,
            r_source_status: &state.r_source_status,
        };
        return match handle_meta_command_result(result, &ctx) {
            MetaAction::Continue => ControlFlow::Continue(()),
            MetaAction::Exit => {
                state.should_exit = true;
                ControlFlow::Break(None)
            }
        };
    }

    finalize_history(Some(history_handle), save_outcome, false);

    // Shell mode: execute as shell command instead of R
    if is_shell_mode {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            // Check if user wants to exit shell mode.
            // We compare commands as strings because Shell mode doesn't run
            // a persistent shell process - each command is executed via
            // `$SHELL -c "command"`. There's no actual shell session to exit,
            // so we intercept "exit" and "logout" to return to R mode instead
            // of running them as no-op shell commands.
            if trimmed == "exit" || trimmed == "logout" {
                state.prompt_config.set_shell(false);
                arf_println!("Returned to R mode.");
                return ControlFlow::Continue(());
            }
            // Show a hint for cd/pushd/popd since they have no effect
            // in a subprocess. The command still runs in the shell.
            if let Some(hint) = dir_command_hint(trimmed) {
                arf_println!("{}", hint);
            }
            execute_shell_command(trimmed);
        }
        return ControlFlow::Continue(());
    }

    // In reprex mode, strip lines starting with "#>" (reprex output comments)
    // This allows users to paste reprex output directly without duplicate output
    // Keep original for line count calculation in clear_input_lines
    let (original_line, line) = if state.reprex.is_enabled() {
        (line.clone(), strip_reprex_output(&line))
    } else {
        (line.clone(), line)
    };

    // Format code when format mode is enabled.
    let code = match state.reprex.maybe_format_code(&line) {
        Ok(code) => code,
        Err(error) => {
            arf_println!("{}", error);
            record_formatter_failure(state, prompt_kind, history_handle, save_outcome);
            return ControlFlow::Continue(());
        }
    };

    // In reprex mode, clear the prompt and input lines
    // Show the (possibly formatted) code
    // Use original_line for line count since that's what was displayed on terminal
    if state.reprex.is_enabled() && !code.is_empty() {
        clear_input_lines(&original_line, &code);
    }

    // Record command start time for the {duration} prompt placeholder
    // Start the spinner to indicate R is evaluating code
    // The spinner will be stopped when R produces output or the next prompt appears
    if !code.is_empty() {
        state.prompt_config.set_command_start();
        state.prompt_config.start_spinner();
    }

    // Mark R as busy (no longer at prompt) for IPC
    crate::ipc::set_r_at_prompt(false);

    // Return the (possibly formatted) code to R
    // Only a top-level command starts a new Reedline history
    // context. Continuation prompts remain part of the outer
    // command, so preserve its context until evaluation ends.
    if prompt_kind.is_command() && !code.trim().is_empty() {
        if crate::ipc::current_execution_id().is_none() {
            crate::ipc::begin_terminal_execution(&code);
        }
        let history_id = save_outcome.and_then(crate::history::HistorySaveOutcome::saved_id);
        state.pending_history_context = PendingHistoryContext::Command {
            store: history_handle.store(),
            history_id,
        };
    }
    ControlFlow::Break(Some(code))
}

/// Return from shell mode to R mode after Ctrl+C or Ctrl+D.
///
/// Returns `false` when the session is not in shell mode.
fn leave_shell_mode(state: &mut ReplState) -> bool {
    if state.prompt_config.is_shell_enabled() {
        state.prompt_config.set_shell(false);
        arf_println!("Returned to R mode.");
        true
    } else {
        false
    }
}

/// Handle Ctrl+C at the prompt.
pub(super) fn handle_ctrl_c(state: &mut ReplState) -> ControlFlow<Option<String>> {
    // Clear any visible completion menu before printing ^C
    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
    println!("^C");
    // In shell mode, Ctrl+C returns to R mode
    if leave_shell_mode(state) {
        return ControlFlow::Continue(());
    }
    ControlFlow::Break(Some(String::new()))
}

/// Handle Ctrl+D at the prompt.
pub(super) fn handle_ctrl_d(state: &mut ReplState) -> ControlFlow<Option<String>> {
    // Clear any visible menu before proceeding
    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
    // In shell mode, Ctrl+D returns to R mode (consistent with Ctrl+C)
    if leave_shell_mode(state) {
        return ControlFlow::Continue(());
    }
    state.should_exit = true;
    ControlFlow::Break(None)
}
