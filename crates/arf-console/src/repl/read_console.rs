//! ReadConsole callback and prompt handling.

use crossterm::{
    ExecutableCommand,
    style::Stylize,
    terminal::{self, ClearType},
};
use reedline::Signal;
use std::io::{self, Write};

use super::history::{finalize_history, save_ipc_history};
use super::state::PendingHistoryContext;
use super::{
    MetaAction, REPL_STATE, RPrompt, SessionInfoContext, arf_println, clear_input_lines,
    execute_shell_command, handle_meta_command_result, meta_command, process_meta_command,
    strip_reprex_output,
};

struct ApprovedInteractiveIpcOperation {
    reply: tokio::sync::oneshot::Sender<crate::ipc::protocol::IpcResponse>,
    wrote_newline: bool,
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

/// ReadConsole callback function.
/// This is called by R when it needs user input.
///
/// With the Validator in place, reedline handles multiline input internally.
/// The callback receives complete expressions (possibly with embedded newlines)
/// from reedline and passes them to R.
pub(super) fn read_console_callback(r_prompt: &str) -> Option<String> {
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

        // Update exit_status for the previous command when a new prompt is shown.
        // This is called when R has finished evaluating and wants new input.
        // Continuation prompts (starting with '+') mean we're still in the same expression.
        // Non-command prompts (menus, etc.) should also not trigger exit status updates.
        // Track prompt state for IPC: true when R is idle at the command
        // prompt, false for continuation/menu/selection prompts so IPC
        // requests are correctly rejected during non-command prompts.
        crate::ipc::set_r_at_prompt(is_r_command_prompt(r_prompt));

        if is_r_command_prompt(r_prompt) && !state.prompt_config.is_shell_enabled() {
            let pending_history_context = std::mem::take(&mut state.pending_history_context);
            let had_error = match pending_history_context {
                PendingHistoryContext::Command { store, history_id } => {
                    let had_error = arf_libr::command_had_error();
                    if let (Some(store), Some(history_id)) = (&store, history_id) {
                        let exit_status = if had_error { 1i64 } else { 0i64 };
                        if let Err(error) = store.set_exit_status(history_id, exit_status) {
                            log::warn!("Failed to update history exit status: {error}");
                        }
                    }

                    if state.forget_config.enabled {
                        let effective_delay = if state.forget_config.on_exit_only {
                            usize::MAX
                        } else {
                            state.forget_config.delay
                        };
                        if let Some(id_to_delete) = state.sponge_queue.record_command(
                            had_error,
                            history_id,
                            effective_delay,
                        ) && let Some(store) = &store
                        {
                            let _ = store.delete(id_to_delete);
                        }
                    }
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

        // Check for pending IPC operations before entering the reedline input loop.
        // At this point reedline hasn't started, so there's no editor buffer to
        // conflict with — we can always accept.
        if is_r_command_prompt(r_prompt)
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
                    run_silent_eval(&op.code, reply);
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
                        return Some(op.code);
                    }
                }
            }
        }

        loop {
            // Build prompt dynamically from config.
            // We detect the type of prompt R is asking for:
            // - Continuation prompts start with '+' (multiline input)
            // - Command prompts typically end with "> " (R's default prompt)
            // - Non-standard prompts (menus, etc.) are passed through directly
            let prompt = if r_prompt.starts_with('+') {
                state.prompt_config.build_cont_prompt()
            } else if is_r_command_prompt(r_prompt) {
                state.prompt_config.build_main_prompt()
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

            // Process R events once before entering the input loop.
            // The idle callback will continue processing events at ~30fps while waiting for input,
            // keeping graphics windows (plot(), help browser) responsive.
            arf_libr::process_r_events();

            // Track whether we're in a non-standard prompt mode (menu selection, etc.)
            let is_menu_prompt = !is_r_command_prompt(r_prompt) && !r_prompt.starts_with('+');

            match editor.read_line(&prompt) {
                Ok(Signal::Success(line)) => {
                    let save_outcome = history_handle.receipt_outcome();

                    // For non-standard prompts (menus, etc.), pass input directly to R
                    // without any processing (meta commands, shell mode, reprex, autoformat)
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
                    let (original_line, line) = if state.prompt_config.is_reprex_enabled() {
                        (line.clone(), strip_reprex_output(&line))
                    } else {
                        (line.clone(), line)
                    };

                    // Format code if autoformat is enabled
                    let code = state.prompt_config.maybe_format_code(&line);

                    // In reprex mode, clear the prompt and input lines
                    // Show the (possibly formatted) code
                    // Use original_line for line count since that's what was displayed on terminal
                    if state.prompt_config.is_reprex_enabled() && !code.is_empty() {
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
                    if is_r_command_prompt(r_prompt) && !code.trim().is_empty() {
                        let history_id = match save_outcome {
                            Some(crate::history::HistorySaveOutcome::Saved(id)) => Some(id),
                            _ => None,
                        };
                        state.pending_history_context = PendingHistoryContext::Command {
                            store: history_handle.store(),
                            history_id,
                        };
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

                            run_silent_eval(&op.code, reply);

                            // Clear the indicator — reedline will repaint the prompt
                            {
                                let mut out = io::stdout();
                                let _ = out.execute(crossterm::cursor::MoveToColumn(0));
                                let _ = out.execute(terminal::Clear(ClearType::CurrentLine));
                            }

                            continue;
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

/// Check if the prompt is R's standard command prompt (top-level).
///
/// Uses R's call stack depth (sys.nframe()) to determine if we're at the top-level
/// or if user code is requesting input (e.g., via readline() or menu()).
///
/// This approach is more robust than heuristics like checking prompt endings,
/// because it detects the actual R evaluation context.
///
/// Returns true if:
/// - We're at the top-level (n_frame == 0) AND not a continuation prompt
///
/// Returns false if:
/// - This is a continuation prompt (starts with '+')
/// - User code is requesting input (n_frame > 0), e.g., readline(), menu()
///
/// Reference: This approach is used by ark (Positron's R kernel).
fn is_r_command_prompt(prompt: &str) -> bool {
    // Continuation prompts (starting with '+') are NOT command prompts
    //
    // TODO: R lets users change this prefix with options(continue = "..."),
    // and a custom value is then misread as a command prompt. Compare against
    // R's configured continuation string instead. Every caller of this
    // function is affected, including IPC request admission.
    if prompt.starts_with('+') {
        return false;
    }

    // Use R's call stack depth to detect if we're at top-level
    // n_frame == 0 means top-level prompt
    // n_frame > 0 means user code is requesting input (readline, menu, etc.)
    match arf_harp::r_n_frame() {
        Ok(n_frame) => n_frame == 0,
        Err(_) => {
            // If we can't get n_frame, fall back to heuristic
            // R's default prompt ends with "> ", menu prompts end with ": "
            prompt.ends_with("> ")
        }
    }
}
