//! ReadConsole callback and prompt handling.

use crate::config::HistoryForgetConfig;
use crate::history::HistoryStore;
use crossterm::{
    ExecutableCommand,
    style::Stylize,
    terminal::{self, ClearType},
};
use reedline::{HistoryItemId, Signal};
use std::ffi::{CStr, c_void};
use std::io::{self, Write};
use std::os::raw::{c_char, c_int};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::history::{finalize_history, save_ipc_history};
use super::state::{
    CommandEvent, CommandId, CommandOrigin, CommandPhase, HistoryEffect, NativeTerminalFact,
    PendingHistoryContext, PromptEffect, SpongeEffect, SpongeQueue, reduce_command,
};
use super::{
    MetaAction, REPL_STATE, RPrompt, SessionInfoContext, arf_eprintln, arf_println,
    clear_input_lines, execute_shell_command, handle_meta_command_result, meta_command,
    process_meta_command, strip_reprex_output,
};

struct ApprovedInteractiveIpcOperation {
    reply: tokio::sync::oneshot::Sender<crate::ipc::protocol::IpcResponse>,
    wrote_newline: bool,
}

static NEXT_COMMAND_ID: AtomicU64 = AtomicU64::new(1);
static PENDING_NESTED_INPUT: Mutex<String> = Mutex::new(String::new());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeInputMode {
    TopLevel,
    Nested,
}

fn native_input_mode(mode: c_int) -> Option<NativeInputMode> {
    match mode {
        value if value == arf_libr::ReplInputMode::TopLevel as c_int => {
            Some(NativeInputMode::TopLevel)
        }
        value if value == arf_libr::ReplInputMode::Nested as c_int => Some(NativeInputMode::Nested),
        _ => None,
    }
}

fn is_no_command_input(input: &str) -> bool {
    input.trim().is_empty()
}

fn native_outcome_matches_command(
    command_id: u64,
    active_command: CommandId,
    pending_history_command: Option<CommandId>,
) -> bool {
    active_command.0 == command_id && pending_history_command == Some(active_command)
}

fn discard_unstarted_history_context() {
    REPL_STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut()
            && let Some(state) = state.as_mut()
        {
            state.pending_history_context = PendingHistoryContext::None;
            state.pending_history_command_id = None;
        }
    });
}

/// Record the outcome for a command that has already been saved by reedline.
///
/// R normally reports this outcome when the next top-level prompt is requested.
/// Formatter failures never reach R, so they use the same persistence and
/// sponge bookkeeping immediately before returning to the input loop.
fn record_command_outcome(
    store: Option<HistoryStore>,
    history_id: Option<HistoryItemId>,
    failed: bool,
    forget_config: &HistoryForgetConfig,
    sponge_queue: &mut SpongeQueue,
) {
    if let (Some(store), Some(history_id)) = (&store, history_id) {
        let exit_status = if failed { 1i64 } else { 0i64 };
        if let Err(error) = store.set_exit_status(history_id, exit_status) {
            log::warn!("Failed to update history exit status: {error}");
        }
    }

    if forget_config.enabled {
        let effective_delay = if forget_config.on_exit_only {
            usize::MAX
        } else {
            forget_config.delay
        };
        if let Some(id_to_delete) = sponge_queue.record_command(failed, history_id, effective_delay)
            && let Some(store) = &store
            && let Err(error) = store.delete(id_to_delete)
        {
            log::warn!("Failed to remove forgotten history entry {id_to_delete}: {error}");
        }
    }
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

pub(super) unsafe extern "C" fn native_top_level_prompt_callback(
    _prompt: *const c_char,
    _context: *mut c_void,
) -> c_int {
    arf_libr::classify_repl_prompt() as c_int
}

pub(super) unsafe extern "C" fn native_input_callback(
    mode: c_int,
    prompt: *const c_char,
    buffer: *mut c_char,
    buffer_len: c_int,
    _history: c_int,
    full_source_out: *mut *mut c_char,
    command_id_out: *mut u64,
    _context: *mut c_void,
) -> c_int {
    let Some(mode) = native_input_mode(mode) else {
        return arf_libr::ReplInputResult::Eof as c_int;
    };
    let start = unsafe { arf_libr::begin_repl_read_console(prompt, buffer, buffer_len) };
    let prompt_info = match start {
        arf_libr::ReplReadConsoleStart::Input(info) => info,
        arf_libr::ReplReadConsoleStart::Askpass(result) => {
            return if result == 0 {
                arf_libr::ReplInputResult::Eof as c_int
            } else {
                arf_libr::ReplInputResult::Text as c_int
            };
        }
    };
    let prompt_text = if prompt.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(prompt) }
            .to_str()
            .unwrap_or_default()
    };
    let is_native_top_level = mode == NativeInputMode::TopLevel;
    let forced_kind = match mode {
        NativeInputMode::TopLevel
            if prompt_info.options_are_ambiguous && prompt_info.is_continuation =>
        {
            PromptKind::Continuation
        }
        NativeInputMode::TopLevel => PromptKind::Command,
        NativeInputMode::Nested => PromptKind::Other,
    };

    if mode == NativeInputMode::Nested {
        let mut pending = PENDING_NESTED_INPUT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !pending.is_empty() {
            return write_nested_input(&mut pending, buffer, buffer_len);
        }
    }

    let input = read_console_callback_impl(
        prompt_text,
        prompt_info,
        Some(forced_kind),
        true,
        is_native_top_level,
    );
    let Some(input) = input else {
        return arf_libr::ReplInputResult::Eof as c_int;
    };

    if mode == NativeInputMode::Nested {
        let mut pending = PENDING_NESTED_INPUT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *pending = input;
        return write_nested_input(&mut pending, buffer, buffer_len);
    }

    let Some((cancelled, origin)) = REPL_STATE.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        let state = state.as_mut()?;
        let cancelled = std::mem::take(&mut state.input_was_cancelled);
        let origin = state
            .next_command_origin
            .take()
            .unwrap_or(CommandOrigin::User);
        Some((cancelled, origin))
    }) else {
        log::error!("Unable to access REPL state after native input callback");
        return arf_libr::ReplInputResult::Cancelled as c_int;
    };

    if cancelled {
        return arf_libr::ReplInputResult::Cancelled as c_int;
    }
    if is_no_command_input(&input) {
        return arf_libr::copy_repl_source("")
            .map(|source| {
                unsafe { *full_source_out = source };
                arf_libr::ReplInputResult::Text as c_int
            })
            .unwrap_or(arf_libr::ReplInputResult::Cancelled as c_int);
    }
    let Some(source) = arf_libr::copy_repl_source(&input) else {
        arf_eprintln!("Error: failed to allocate R command input");
        discard_unstarted_history_context();
        return arf_libr::ReplInputResult::Cancelled as c_int;
    };
    let id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed).max(1);
    unsafe { *full_source_out = source };
    let id = CommandId(id);
    let reduction = reduce_command(None, CommandEvent::Accepted { id, origin });
    let started = reduce_command(reduction.lifecycle, CommandEvent::Started { id });
    let started = reduce_command(
        started.lifecycle,
        CommandEvent::PhaseChanged {
            id,
            phase: CommandPhase::Parse,
        },
    );
    let stored = REPL_STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut()
            && let Some(state) = state.as_mut()
        {
            state.command_lifecycle = started.lifecycle;
            state.pending_history_command_id = Some(id);
            return true;
        }
        false
    });
    if !stored {
        log::error!("Unable to store accepted native R command lifecycle");
        discard_unstarted_history_context();
        return arf_libr::ReplInputResult::Cancelled as c_int;
    }
    unsafe { *command_id_out = id.0 };
    arf_libr::ReplInputResult::Text as c_int
}

fn write_nested_input(input: &mut String, buffer: *mut c_char, buffer_len: c_int) -> c_int {
    if buffer.is_null() || buffer_len <= 2 {
        return arf_libr::ReplInputResult::Eof as c_int;
    }
    let max = (buffer_len as usize).saturating_sub(2);
    let mut end = input.len().min(max);
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut chunk = input.drain(..end).collect::<String>();
    if input.is_empty() {
        chunk.push('\n');
    }
    unsafe {
        std::ptr::copy_nonoverlapping(chunk.as_ptr(), buffer.cast::<u8>(), chunk.len());
        buffer.add(chunk.len()).write(0);
    }
    arf_libr::ReplInputResult::Text as c_int
}

pub(super) unsafe extern "C" fn native_outcome_callback(
    command_id: u64,
    _expression_id: u32,
    fact: u8,
    _context: *mut c_void,
) {
    if command_id == 0 {
        return;
    }
    let Some(outcome) = arf_libr::ReplOutcome::from_native(command_id, _expression_id, fact) else {
        return;
    };
    let fact = match outcome.fact {
        arf_libr::ReplFact::Completed => NativeTerminalFact::Completed,
        arf_libr::ReplFact::AbortedParse => NativeTerminalFact::Aborted(CommandPhase::Parse),
        arf_libr::ReplFact::AbortedEval => NativeTerminalFact::Aborted(CommandPhase::Eval),
        arf_libr::ReplFact::AbortedPrint => NativeTerminalFact::Aborted(CommandPhase::Print),
        arf_libr::ReplFact::Unobserved => NativeTerminalFact::Unobserved,
    };
    REPL_STATE.with(|state| {
        let Ok(mut borrowed) = state.try_borrow_mut() else {
            return;
        };
        let Some(state) = borrowed.as_mut() else {
            return;
        };
        let Some(lifecycle) = state.command_lifecycle else {
            return;
        };
        let id = lifecycle.id;
        if !native_outcome_matches_command(command_id, id, state.pending_history_command_id) {
            log::warn!("Ignoring stale native R outcome for command {command_id}");
            return;
        }
        let awaiting = reduce_command(Some(lifecycle), CommandEvent::AwaitingTopLevel { id });
        let terminal = reduce_command(
            awaiting.lifecycle,
            CommandEvent::NativeTerminal { id, fact },
        );
        let Some(finalized) = terminal.finalized else {
            return;
        };
        state.command_lifecycle = terminal.lifecycle;
        state.pending_history_command_id = None;
        PENDING_NESTED_INPUT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        arf_libr::stop_spinner();
        crate::ipc::set_r_at_prompt(true);

        let projection = finalized.consumer_projection();
        match projection.prompt {
            PromptEffect::SetSuccess => state.prompt_config.set_last_command_failed(false),
            PromptEffect::SetFailure => state.prompt_config.set_last_command_failed(true),
            PromptEffect::Keep => {}
        }
        if !matches!(projection.prompt, PromptEffect::Keep) {
            state.prompt_config.set_command_duration();
        }
        let pending = std::mem::take(&mut state.pending_history_context);
        let (store, history_id) = match pending {
            PendingHistoryContext::Command { store, history_id } => (store, history_id),
            PendingHistoryContext::None => (None, None),
        };
        match (projection.history, projection.sponge) {
            (HistoryEffect::Success, SpongeEffect::RecordSuccess) => record_command_outcome(
                store,
                history_id,
                false,
                &state.forget_config,
                &mut state.sponge_queue,
            ),
            (HistoryEffect::Failure, SpongeEffect::RecordFailure) => record_command_outcome(
                store,
                history_id,
                true,
                &state.forget_config,
                &mut state.sponge_queue,
            ),
            _ => {}
        }
    });
}

/// ReadConsole callback function.
/// This is called by R when it needs user input.
///
/// With the Validator in place, reedline handles multiline input internally.
/// The callback receives complete expressions (possibly with embedded newlines)
/// from reedline and passes them to R.
#[allow(dead_code)] // Retained as the legacy path for the next cleanup slice.
pub(super) fn read_console_callback(
    r_prompt: &str,
    prompt_info: arf_libr::ReadConsolePromptInfo,
) -> Option<String> {
    read_console_callback_impl(r_prompt, prompt_info, None, false, false)
}

fn read_console_callback_impl(
    r_prompt: &str,
    prompt_info: arf_libr::ReadConsolePromptInfo,
    forced_prompt_kind: Option<PromptKind>,
    native_driver: bool,
    native_top_level: bool,
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
        let prompt_kind = forced_prompt_kind
            .unwrap_or_else(|| classify_prompt(r_prompt, prompt_info.is_continuation));
        let is_command_prompt = if native_driver {
            native_top_level
        } else {
            prompt_kind.is_command()
        };

        // Update exit_status for the previous command when a new prompt is shown.
        // This is called when R has finished evaluating and wants new input.
        // Continuation prompts identified by the low-level option lookup mean
        // we're still in the same expression.
        // Non-command prompts (menus, etc.) should also not trigger exit status updates.
        // Track prompt state for IPC: true when R is idle at the command
        // prompt, false for continuation/menu/selection prompts so IPC
        // requests are correctly rejected during non-command prompts.
        crate::ipc::set_r_at_prompt(is_command_prompt);

        if is_command_prompt && !state.prompt_config.is_shell_enabled() && !native_driver {
            let pending_history_context = std::mem::take(&mut state.pending_history_context);
            let had_error = match pending_history_context {
                PendingHistoryContext::Command { store, history_id } => {
                    let had_error = arf_libr::command_had_error();
                    record_command_outcome(
                        store,
                        history_id,
                        had_error,
                        &state.forget_config,
                        &mut state.sponge_queue,
                    );
                    had_error
                }
                PendingHistoryContext::None => false,
            };

            // Update prompt status indicator for the next prompt
            state.prompt_config.set_last_command_failed(had_error);

            // Calculate duration for the {duration} prompt placeholder
            state.prompt_config.set_command_duration();

            // Reset error state for the next command
            arf_libr::reset_command_error_state();
        }

        if is_command_prompt {
            state.input_was_cancelled = false;
        }

        // Check for pending IPC operations before entering the reedline input loop.
        // At this point reedline hasn't started, so there's no editor buffer to
        // conflict with — we can always accept.
        if is_command_prompt
            && !state.prompt_config.is_shell_enabled()
            && let Some(op) = crate::ipc::take_pending_ipc_operation()
        {
            use crate::ipc::{
                PendingIpcKind, accept_user_input, run_silent_eval, setup_visible_eval,
            };
            match op.kind {
                PendingIpcKind::SilentEvaluate { reply } => {
                    // Run silent evaluate directly — no buffer conflict possible.
                    // Unlike visible eval / user_input, silent eval does not return
                    // code to R. It runs synchronously here and then falls through
                    // to the reedline loop below to wait for user input.
                    drop(guard);
                    run_silent_eval(&op.code, reply);
                    return read_console_callback_impl(
                        r_prompt,
                        prompt_info,
                        forced_prompt_kind,
                        native_driver,
                        native_top_level,
                    );
                }
                PendingIpcKind::VisibleEvaluate { reply, timeout } => {
                    if let Some(ApprovedInteractiveIpcOperation { reply, .. }) =
                        approve_interactive_ipc_operation(&op.code, reply)
                    {
                        setup_visible_eval(reply, timeout);
                        let store = state.r_history.store();
                        let history_id = save_ipc_history(
                            state.line_editor.history_mut(),
                            store,
                            &op.code,
                            state.history_session_id,
                        );
                        if !op.code.trim().is_empty() {
                            state.pending_history_context = PendingHistoryContext::Command {
                                store: state.r_history.store(),
                                history_id,
                            };
                        }
                        let prompt_str = "agent> ";
                        println!("{}{}", prompt_str.dark_cyan(), op.code);
                        if !op.code.is_empty() {
                            state.prompt_config.set_command_start();
                            state.prompt_config.start_spinner();
                        }
                        crate::ipc::set_r_at_prompt(false);
                        state.next_command_origin = Some(CommandOrigin::VisibleIpc);
                        return Some(op.code);
                    }
                }
                PendingIpcKind::UserInput { reply } => {
                    if let Some(ApprovedInteractiveIpcOperation { reply, .. }) =
                        approve_interactive_ipc_operation(&op.code, reply)
                    {
                        accept_user_input(reply);
                        let store = state.r_history.store();
                        let history_id = save_ipc_history(
                            state.line_editor.history_mut(),
                            store,
                            &op.code,
                            state.history_session_id,
                        );
                        if !op.code.trim().is_empty() {
                            state.pending_history_context = PendingHistoryContext::Command {
                                store: state.r_history.store(),
                                history_id,
                            };
                        }
                        let prompt_str = "agent> ";
                        println!("{}{}", prompt_str.dark_cyan(), op.code);
                        if !op.code.is_empty() {
                            state.prompt_config.set_command_start();
                            state.prompt_config.start_spinner();
                        }
                        crate::ipc::set_r_at_prompt(false);
                        state.next_command_origin = Some(CommandOrigin::VisibleIpc);
                        return Some(op.code);
                    }
                }
            }
        }

        loop {
            // Build prompt dynamically from config.
            // We detect the type of prompt R is asking for:
            // - Continuation prompts are identified by PromptKind (multiline input)
            // - Command prompts are identified by PromptKind at R's top level
            // - Non-standard prompts (menus, etc.) are passed through directly
            let prompt = if prompt_kind.is_continuation() {
                state.prompt_config.build_cont_prompt(state.reprex.mode)
            } else if prompt_kind.is_command() {
                state.prompt_config.build_main_prompt(state.reprex.mode)
            } else {
                // Non-standard prompt from R (menu selection, etc.)
                // Pass through R's actual prompt instead of our configured one
                RPrompt::new(r_prompt.to_string(), r_prompt.to_string())
            };

            // Use shell editor when in shell mode (for separate history)
            let is_shell_mode = state.prompt_config.is_shell_enabled();
            let history_handle = if is_shell_mode {
                state.shell_history.clone()
            } else {
                state.r_history.clone()
            };
            let editor = if is_shell_mode {
                &mut state.shell_line_editor
            } else {
                &mut state.line_editor
            };

            // Track whether we're in a non-standard prompt mode (menu selection, etc.)
            let is_menu_prompt = prompt_kind.is_other();

            // Event processing and the terminal wait both run while the REPL
            // state is borrowed. Keep R's interrupt forwarding disabled for
            // the whole interval so an R longjmp cannot cross these Rust frames.
            let read_result = arf_libr::with_repl_input_guard(|| {
                // Process R events once before entering the input loop. The idle
                // callback continues at ~30fps while reedline is waiting.
                arf_libr::process_r_events();
                editor.read_line(&prompt)
            });
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

            match read_result {
                Ok(Signal::Success(line)) => {
                    let save_outcome = history_handle.receipt_outcome();

                    // For non-standard prompts (menus, etc.), pass input directly to R
                    // without any processing (meta commands, shell mode, or reprex)
                    if is_menu_prompt {
                        // Deliberately leave pending_history_context alone. This
                        // input was requested by R during an evaluation that is
                        // already in progress (readline(), menu(), browser()),
                        // so the outer command still owns the result. Claiming
                        // the context here would strand the outer entry without
                        // an exit status, which matters most when that entry
                        // came from IPC and cannot be recovered from reedline's
                        // own last-command context.
                        finalize_history(Some(&history_handle), save_outcome, false);
                        return Some(line);
                    }

                    // Check for meta commands first
                    if let Some(result) = process_meta_command(
                        &line,
                        &mut state.prompt_config,
                        &mut state.reprex,
                        &state.r_history,
                        &state.shell_history,
                        &state.r_source_status,
                        &mut state.dir_stack,
                        state.history_session_id.map(i64::from),
                        state.r_home.as_deref(),
                    ) {
                        finalize_history(Some(&history_handle), save_outcome, true);
                        // Clear duration so the previous R command's time
                        // does not persist in the prompt after a meta command.
                        state.prompt_config.clear_command_duration();
                        let ctx = SessionInfoContext {
                            prompt_config: &state.prompt_config,
                            reprex: &state.reprex,
                            config_path: &state.config_path,
                            config_status: state.config_status,
                            r_history: &state.r_history,
                            shell_history: &state.shell_history,
                            r_source_status: &state.r_source_status,
                        };
                        match handle_meta_command_result(result, &ctx) {
                            MetaAction::Continue => continue,
                            MetaAction::Exit => {
                                state.should_exit = true;
                                return None;
                            }
                        }
                    }

                    finalize_history(Some(&history_handle), save_outcome, false);

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
                                continue;
                            }
                            // Show a hint for cd/pushd/popd since they have no effect
                            // in a subprocess. The command still runs in the shell.
                            if let Some(hint) = meta_command::dir_command_hint(trimmed) {
                                arf_println!("{}", hint);
                            }
                            execute_shell_command(trimmed);
                        }
                        continue;
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
                            // A continuation input still belongs to the outer
                            // command. It must not finalize that command's
                            // history or sponge state merely because formatting
                            // this piece failed.
                            if formatter_failure_updates_lifecycle(is_command_prompt) {
                                let history_id = match save_outcome {
                                    Some(crate::history::HistorySaveOutcome::Saved(id)) => Some(id),
                                    _ => None,
                                };
                                record_command_outcome(
                                    history_handle.store(),
                                    history_id,
                                    true,
                                    &state.forget_config,
                                    &mut state.sponge_queue,
                                );
                                state.prompt_config.set_last_command_failed(true);
                                state.prompt_config.clear_command_duration();
                            }
                            continue;
                        }
                    };

                    // In reprex mode, clear the prompt and input lines
                    // Show the (possibly formatted) code
                    // Use original_line for line count since that's what was displayed on terminal
                    if state.reprex.is_enabled() && !code.is_empty() {
                        clear_input_lines(&original_line, &code);
                    }

                    // Formatting can produce an expression that is incomplete
                    // even when the user's original line was complete. Collect
                    // continuation lines in the UI before returning the full
                    // source to the native driver, which evaluates only after
                    // this callback has released its Rust state borrow.
                    let mut code = code;
                    if native_driver && is_command_prompt {
                        crate::ipc::set_r_at_prompt(false);
                        let validator = crate::editor::validator::RValidator::new();
                        while !validator.is_complete(&code) {
                            let continuation_prompt =
                                state.prompt_config.build_cont_prompt(state.reprex.mode);
                            let continuation_result = arf_libr::with_repl_input_guard(|| {
                                arf_libr::process_r_events();
                                editor.read_line(&continuation_prompt)
                            });
                            match continuation_result {
                                Ok(Signal::Success(line)) => {
                                    let continuation_save = history_handle.receipt_outcome();
                                    finalize_history(
                                        Some(&history_handle),
                                        continuation_save,
                                        false,
                                    );
                                    if let Some(result) = process_meta_command(
                                        &line,
                                        &mut state.prompt_config,
                                        &mut state.reprex,
                                        &state.r_history,
                                        &state.shell_history,
                                        &state.r_source_status,
                                        &mut state.dir_stack,
                                        state.history_session_id.map(i64::from),
                                        state.r_home.as_deref(),
                                    ) {
                                        state.prompt_config.clear_command_duration();
                                        let ctx = SessionInfoContext {
                                            prompt_config: &state.prompt_config,
                                            reprex: &state.reprex,
                                            config_path: &state.config_path,
                                            config_status: state.config_status,
                                            r_history: &state.r_history,
                                            shell_history: &state.shell_history,
                                            r_source_status: &state.r_source_status,
                                        };
                                        match handle_meta_command_result(result, &ctx) {
                                            MetaAction::Continue => continue,
                                            MetaAction::Exit => {
                                                state.should_exit = true;
                                                return None;
                                            }
                                        }
                                    }

                                    let continuation = if state.reprex.is_enabled() {
                                        strip_reprex_output(&line)
                                    } else {
                                        line
                                    };
                                    let continuation = match state
                                        .reprex
                                        .maybe_format_code(&continuation)
                                    {
                                        Ok(code) => code,
                                        Err(error) => {
                                            arf_println!("{}", error);
                                            continue;
                                        }
                                    };
                                    if state.reprex.is_enabled() && !continuation.is_empty() {
                                        clear_input_lines(&continuation, &continuation);
                                    }
                                    code.push('\n');
                                    code.push_str(&continuation);
                                }
                                Ok(Signal::CtrlC) => {
                                    let _ = io::stdout()
                                        .execute(terminal::Clear(ClearType::FromCursorDown));
                                    println!("^C");
                                    state.input_was_cancelled = true;
                                    return Some(String::new());
                                }
                                Ok(Signal::CtrlD) => {
                                    state.should_exit = true;
                                    return None;
                                }
                                Ok(Signal::ExternalBreak(buffer)) => {
                                    if let Some(op) = crate::ipc::take_pending_ipc_operation() {
                                        crate::ipc::reject_operation_user_typing(op, &code);
                                    } else if !buffer.trim().is_empty() {
                                        log::debug!("Ignoring external break during continuation input");
                                    }
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    eprintln!("Error: {error}");
                                    state.should_exit = true;
                                    return None;
                                }
                            }
                        }
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
                    if is_command_prompt && !code.trim().is_empty() {
                        let history_id = match save_outcome {
                            Some(crate::history::HistorySaveOutcome::Saved(id)) => Some(id),
                            _ => None,
                        };
                        state.pending_history_context = PendingHistoryContext::Command {
                            store: history_handle.store(),
                            history_id,
                        };
                    }
                    if is_command_prompt {
                        state.next_command_origin = Some(CommandOrigin::User);
                    }
                    return Some(code);
                }
                Ok(Signal::CtrlC) => {
                    // Clear any visible completion menu before printing ^C
                    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
                    println!("^C");
                    // In shell mode, Ctrl+C returns to R mode
                    if state.prompt_config.is_shell_enabled() {
                        state.prompt_config.set_shell(false);
                        arf_println!("Returned to R mode.");
                        continue;
                    }
                    if is_command_prompt {
                        state.input_was_cancelled = true;
                    }
                    return Some(String::new());
                }
                Ok(Signal::CtrlD) => {
                    // Clear any visible menu before proceeding
                    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
                    // In shell mode, Ctrl+D returns to R mode (consistent with Ctrl+C)
                    if state.prompt_config.is_shell_enabled() {
                        state.prompt_config.set_shell(false);
                        arf_println!("Returned to R mode.");
                        continue;
                    }
                    state.should_exit = true;
                    return None;
                }
                Ok(Signal::ExternalBreak(buffer)) => {
                    // IPC operation triggered a break signal.
                    // Check the editor buffer for mutual exclusion with console input.
                    if let Some(op) = crate::ipc::take_pending_ipc_operation() {
                        use crate::ipc::{
                            PendingIpcKind, accept_user_input, reject_operation_user_typing,
                            run_silent_eval, setup_visible_eval,
                        };

                        // If the user has typed something, reject the IPC operation.
                        // We use trim() because reedline may include trailing whitespace
                        // in the buffer; whitespace-only input is treated as empty.
                        if !buffer.trim().is_empty() {
                            reject_operation_user_typing(op, &buffer);
                            continue;
                        }

                        // Helper: clear the current prompt line and show agent prefix
                        let clear_and_show_agent_prompt = |code: &str| {
                            let mut out = io::stdout();
                            let _ = out.execute(crossterm::cursor::MoveToColumn(0));
                            let _ = out.execute(terminal::Clear(ClearType::CurrentLine));
                            println!("{}{}", "agent> ".dark_cyan(), code);
                        };

                        // Silent evaluate: run in-place and return to reedline
                        if let PendingIpcKind::SilentEvaluate { reply } = op.kind {
                            // Show visual indicator, run eval, then return to reedline
                            {
                                let mut out = io::stdout();
                                let _ = out.execute(crossterm::cursor::MoveToColumn(0));
                                let _ = out.execute(terminal::Clear(ClearType::CurrentLine));
                                print!("{}", "[evaluating...]".dark_cyan());
                                let _ = out.flush();
                            }

                            drop(guard);
                            run_silent_eval(&op.code, reply);

                            // Clear the indicator — reedline will repaint the prompt
                            {
                                let mut out = io::stdout();
                                let _ = out.execute(crossterm::cursor::MoveToColumn(0));
                                let _ = out.execute(terminal::Clear(ClearType::CurrentLine));
                            }

                            return read_console_callback_impl(
                                r_prompt,
                                prompt_info,
                                forced_prompt_kind,
                                native_driver,
                                native_top_level,
                            );
                        }

                        // Visible evaluate / user input: accept, inject code into REPL.
                        // Preserve whether approval already emitted its CRLF.
                        let approval_wrote_newline = match op.kind {
                            PendingIpcKind::VisibleEvaluate { reply, timeout } => {
                                let Some(ApprovedInteractiveIpcOperation {
                                    reply,
                                    wrote_newline,
                                }) = approve_interactive_ipc_operation(&op.code, reply)
                                else {
                                    continue;
                                };
                                setup_visible_eval(reply, timeout);
                                wrote_newline
                            }
                            PendingIpcKind::UserInput { reply } => {
                                let Some(ApprovedInteractiveIpcOperation {
                                    reply,
                                    wrote_newline,
                                }) = approve_interactive_ipc_operation(&op.code, reply)
                                else {
                                    continue;
                                };
                                accept_user_input(reply);
                                wrote_newline
                            }
                            PendingIpcKind::SilentEvaluate { .. } => unreachable!(),
                        };

                        let history_id = save_ipc_history(
                            editor.history_mut(),
                            history_handle.store(),
                            &op.code,
                            state.history_session_id,
                        );
                        if !op.code.trim().is_empty() {
                            state.pending_history_context = PendingHistoryContext::Command {
                                store: history_handle.store(),
                                history_id,
                            };
                        }

                        clear_and_show_agent_prompt(&op.code);

                        // ExternalBreak leaves reedline's previous prompt position suspended.
                        // Its saved row range includes the line after a single-line prompt, so
                        // leave the cursor one row beyond that range. Otherwise, when R produces
                        // no output, the next repaint reuses the old prompt origin and clears the
                        // echoed agent line.
                        if !approval_wrote_newline {
                            println!();
                        }

                        if !op.code.is_empty() {
                            state.prompt_config.set_command_start();
                            state.prompt_config.start_spinner();
                        }

                        crate::ipc::set_r_at_prompt(false);
                        if is_command_prompt {
                            state.next_command_origin = Some(CommandOrigin::VisibleIpc);
                        }
                        return Some(op.code);
                    }
                    // No pending operation (spurious signal), continue waiting
                    continue;
                }
                Ok(_) => continue,
                Err(err) => {
                    eprintln!("Error: {}", err);
                    state.should_exit = true;
                    return None;
                }
            }
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Command,
    Continuation,
    Other,
}

impl PromptKind {
    fn is_command(self) -> bool {
        matches!(self, Self::Command)
    }

    fn is_continuation(self) -> bool {
        matches!(self, Self::Continuation)
    }

    fn is_other(self) -> bool {
        matches!(self, Self::Other)
    }
}

/// Classify a ReadConsole prompt once per callback.
///
/// Continuation status comes directly from R's low-level option lookup. For
/// every other prompt, call sys.nframe() once to distinguish a top-level command
/// prompt from input requested by user code (readline(), menu(), browser(), …).
fn classify_prompt(prompt: &str, is_continuation: bool) -> PromptKind {
    if is_continuation {
        return PromptKind::Continuation;
    }

    match arf_harp::r_n_frame() {
        Ok(0) => PromptKind::Command,
        Ok(_) => PromptKind::Other,
        Err(_) => {
            // If R's stack inspection fails, preserve the historical fallback
            // for the standard prompt and treat other prompts conservatively.
            if prompt.ends_with("> ") {
                PromptKind::Command
            } else {
                PromptKind::Other
            }
        }
    }
}

/// Formatter failures only finalize lifecycle state for top-level commands.
/// Continuation prompts belong to the outer command and retain its context.
fn formatter_failure_updates_lifecycle(is_command_prompt: bool) -> bool {
    is_command_prompt
}

/// Warn once for each transition into an ambiguous prompt-option state.
fn should_warn_prompt_ambiguity(was_ambiguous: &mut bool, is_ambiguous: bool) -> bool {
    let should_warn = is_ambiguous && !*was_ambiguous;
    *was_ambiguous = is_ambiguous;
    should_warn
}

#[cfg(test)]
mod tests {
    use super::{
        NativeInputMode, PromptKind, classify_prompt, formatter_failure_updates_lifecycle,
        is_no_command_input, native_input_mode, native_outcome_matches_command,
        should_warn_prompt_ambiguity, write_nested_input,
    };
    use std::ffi::CStr;
    use std::os::raw::c_char;

    #[test]
    fn continuation_formatter_failure_keeps_outer_lifecycle_context() {
        assert!(!formatter_failure_updates_lifecycle(false));
        assert!(formatter_failure_updates_lifecycle(true));
    }

    #[test]
    fn prompt_classification_prioritizes_low_level_continuation_status() {
        assert_eq!(classify_prompt("... ", true), PromptKind::Continuation,);
    }

    #[test]
    fn prompt_classification_falls_back_for_uninitialized_r() {
        assert_eq!(classify_prompt("> ", false), PromptKind::Command,);
        assert_eq!(classify_prompt("Selection: ", false), PromptKind::Other,);
    }

    #[test]
    fn prompt_ambiguity_warning_is_rearmed_after_distinct_options() {
        let mut was_ambiguous = false;

        assert!(should_warn_prompt_ambiguity(&mut was_ambiguous, true));
        assert!(!should_warn_prompt_ambiguity(&mut was_ambiguous, true));
        assert!(!should_warn_prompt_ambiguity(&mut was_ambiguous, false));
        assert!(should_warn_prompt_ambiguity(&mut was_ambiguous, true));
    }

    #[test]
    fn native_input_modes_reject_unknown_values() {
        assert_eq!(
            native_input_mode(arf_libr::ReplInputMode::TopLevel as i32),
            Some(NativeInputMode::TopLevel)
        );
        assert_eq!(
            native_input_mode(arf_libr::ReplInputMode::Nested as i32),
            Some(NativeInputMode::Nested)
        );
        assert_eq!(native_input_mode(-1), None);
    }

    #[test]
    fn whitespace_is_no_command_but_comment_input_is_a_command() {
        assert!(is_no_command_input(" \t\n"));
        assert!(!is_no_command_input("# a comment"));
    }

    #[test]
    fn native_outcome_requires_matching_command_and_history_identity() {
        let id = super::CommandId(17);
        assert!(native_outcome_matches_command(17, id, Some(id)));
        assert!(!native_outcome_matches_command(18, id, Some(id)));
        assert!(!native_outcome_matches_command(17, id, None));
        assert!(!native_outcome_matches_command(
            17,
            id,
            Some(super::CommandId(18))
        ));
    }

    #[test]
    fn nested_input_chunks_preserve_utf8_and_terminate_only_at_the_end() {
        let mut input = "あbc".to_string();
        let mut buffer = [0 as c_char; 6];

        assert_eq!(write_nested_input(&mut input, buffer.as_mut_ptr(), 6), 1);
        assert_eq!(
            unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap(),
            "あb"
        );
        assert_eq!(input, "c");

        assert_eq!(write_nested_input(&mut input, buffer.as_mut_ptr(), 6), 1);
        assert_eq!(
            unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap(),
            "c\n"
        );
        assert!(input.is_empty());
    }

    #[test]
    fn undersized_nested_buffer_does_not_consume_input() {
        let mut input = "keep".to_string();
        let mut buffer = [0 as c_char; 2];

        assert_eq!(write_nested_input(&mut input, buffer.as_mut_ptr(), 2), 0);
        assert_eq!(input, "keep");
    }
}
