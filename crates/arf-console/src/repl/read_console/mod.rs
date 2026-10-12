//! ReadConsole callback and prompt handling.

mod input;
mod ipc;
mod lifecycle;
mod prompt_kind;
#[cfg(test)]
mod tests;

use super::state::ReplState;
use super::{REPL_STATE, RPrompt, arf_eprintln};
use crate::history::HistoryRuntime;
use input::{handle_ctrl_c, handle_ctrl_d, handle_line};
use ipc::{dispatch_pre_prompt_ipc, handle_external_break};
use lifecycle::{finish_previous_command, initialize_error_handler_once};
use prompt_kind::{
    PromptKind, classify_prompt, should_process_pending_help_requests, should_warn_prompt_ambiguity,
};
use reedline::Signal;
use std::ops::ControlFlow;

/// ReadConsole callback function.
/// This is called by R when it needs user input.
///
/// With the Validator in place, reedline handles multiline input internally.
/// The callback receives complete expressions (possibly with embedded newlines)
/// from reedline and passes them to R.
pub(super) fn read_console_callback(
    r_prompt: &str,
    prompt_info: arf_libr::ReadConsolePromptInfo,
) -> Option<String> {
    REPL_STATE.with(|state| {
        // Use try_borrow_mut to detect re-entrant calls.
        // This is a defensive measure in case R unexpectedly calls ReadConsole
        // while we're still processing a previous call. This was originally
        // needed when RValidator called harp::is_expression_complete (which
        // invokes R's parser), but is now less critical since we switched to
        // a tree-sitter-r based validator that doesn't call into R.
        let mut guard = match state.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => {
                // Re-entrant call detected - RefCell already borrowed.
                // Return None (EOF) to terminate the nested call.
                // This prevents panic from double borrow.
                return None;
            }
        };
        let state = guard.as_mut()?;

        if state.should_exit {
            return None;
        }

        if should_warn_prompt_ambiguity(
            &mut state.r_prompt_options_ambiguous,
            prompt_info.options_are_ambiguous,
        ) {
            arf_eprintln!(
                r#"Warning: options("prompt") and options("continue") are identical; arf cannot distinguish top-level and continuation prompts."#
            );
        }

        // The low-level ReadConsole callback reads both prompt options once and
        // supplies their metadata here. Classify the prompt once so every
        // lifecycle, IPC, display, menu, formatter, and history decision shares
        // one value.
        let prompt_kind = classify_prompt(r_prompt, prompt_info.is_continuation);

        initialize_error_handler_once(state, prompt_kind);

        // Track prompt state for IPC: true when R is idle at the command
        // prompt, false for continuation/menu/selection prompts so IPC
        // requests are correctly rejected during non-command prompts.
        crate::ipc::set_r_at_prompt(prompt_kind.is_command());

        finish_previous_command(state, prompt_kind);

        // Prepared help is consumed only at a verified command prompt, after
        // the previous command's lifecycle has been finalized and before IPC
        // dispatch or reedline starts reading the next command.
        if should_process_pending_help_requests(prompt_kind, state.prompt_config.is_shell_enabled())
        {
            super::pager_ui::process_pending_help_requests();
        }

        if let Some(code) = dispatch_pre_prompt_ipc(state, prompt_kind) {
            return Some(code);
        }

        read_input_loop(state, r_prompt, prompt_kind)
    })
}

/// Build the prompt for the current mode.
///
/// We detect the type of prompt R is asking for:
/// - Continuation prompts are identified by PromptKind (multiline input)
/// - Command prompts are identified by PromptKind at R's top level
/// - Non-standard prompts (menus, etc.) are passed through directly
fn build_prompt(state: &ReplState, r_prompt: &str, prompt_kind: PromptKind) -> RPrompt {
    if prompt_kind.is_continuation() {
        state.prompt_config.build_cont_prompt(state.reprex.mode)
    } else if prompt_kind.is_command() {
        state.prompt_config.build_main_prompt(state.reprex.mode)
    } else {
        // Non-standard prompt from R (menu selection, etc.)
        // Pass through R's actual prompt instead of our configured one
        RPrompt::new(r_prompt.to_string(), r_prompt.to_string())
    }
}

/// Read from the line editor until there is input to hand to R, or the
/// session ends.
fn read_input_loop(
    state: &mut ReplState,
    r_prompt: &str,
    prompt_kind: PromptKind,
) -> Option<String> {
    loop {
        // Build prompt dynamically from config.
        let prompt = build_prompt(state, r_prompt, prompt_kind);

        // Use shell editor when in shell mode (for separate history)
        let is_shell_mode = state.prompt_config.is_shell_enabled();
        let history_handle: HistoryRuntime = if is_shell_mode {
            state.shell_history.clone()
        } else {
            state.r_history.clone()
        };
        let editor = if is_shell_mode {
            &mut state.shell_line_editor
        } else {
            &mut state.line_editor
        };

        // Process R events once before entering the input loop.
        // The idle callback will continue processing events at ~30fps while waiting for input,
        // keeping graphics windows (plot(), help browser) responsive.
        arf_libr::process_r_events();

        let read_result = editor.read_line(&prompt);
        // Keep startup echo suppression through the raw-mode transition.
        // The original cooked mode is restored only after reedline returns,
        // so early PTY input cannot pass through an echo window.
        if read_result.is_ok()
            && let Err(error) = crate::console_mode::handoff_to_reedline()
        {
            eprintln!("Error: {}", error);
            state.should_exit = true;
            return None;
        }

        let step = match read_result {
            Ok(Signal::Success(line)) => {
                handle_line(state, &history_handle, prompt_kind, is_shell_mode, line)
            }
            Ok(Signal::CtrlC) => handle_ctrl_c(state),
            Ok(Signal::CtrlD) => handle_ctrl_d(state),
            // IPC operation triggered a break signal.
            Ok(Signal::ExternalBreak(buffer)) => {
                handle_external_break(state, &history_handle, is_shell_mode, &buffer)
            }
            Ok(_) => ControlFlow::Continue(()),
            Err(err) => {
                eprintln!("Error: {}", err);
                state.should_exit = true;
                return None;
            }
        };
        if let ControlFlow::Break(result) = step {
            return result;
        }
    }
}
