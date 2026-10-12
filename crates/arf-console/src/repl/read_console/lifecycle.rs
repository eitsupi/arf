//! Command lifecycle bookkeeping around each ReadConsole request: R error
//! handler setup, exit status, duration, and sponge tracking of the previous
//! command.

use super::prompt_kind::{PromptKind, formatter_failure_updates_lifecycle};
use crate::config::HistoryForgetConfig;
use crate::history::{HistoryRuntime, HistorySaveOutcome, HistoryStore};
use crate::repl::state::{PendingHistoryContext, ReplState, SpongeQueue};
use reedline::HistoryItemId;

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

/// Project a reliable R outcome into the prompt, history, and sponge lifecycle.
/// An unavailable outcome has no reliable boolean projection.
pub(super) fn outcome_failure_projection(outcome: arf_libr::CommandOutcome) -> Option<bool> {
    match outcome {
        arf_libr::CommandOutcome::Success => Some(false),
        arf_libr::CommandOutcome::Failure => Some(true),
        arf_libr::CommandOutcome::Unavailable => None,
    }
}

/// Wrap R's error handler once, at the first real command prompt.
///
/// R startup and user profiles can replace options(error) after
/// initialize_r returns. Capture and wrap the resulting handler at the
/// first real command prompt, before returning any user input to R.
pub(super) fn initialize_error_handler_once(state: &mut ReplState, prompt_kind: PromptKind) {
    if prompt_kind.is_command() && !state.error_handler_setup_attempted {
        state.error_handler_setup_attempted = true;
        if !arf_libr::command_outcome_tracking_available() {
            log::info!(
                "R library lacks safe error-state inspection; command outcomes are unavailable"
            );
        } else {
            match arf_harp::eval_string_in_base(arf_libr::global_error_handler_code()) {
                Ok(_) => {
                    log::info!("R error handler initialized after startup");
                    arf_libr::mark_global_error_handler_initialized();
                }
                Err(error) => {
                    log::warn!("Failed to initialize R error handler: {error:?}");
                }
            }
        }
    }
}

/// Update exit_status for the previous command when a new prompt is shown.
/// This is called when R has finished evaluating and wants new input.
/// Continuation prompts identified by the low-level option lookup mean
/// we're still in the same expression.
/// Non-command prompts (menus, etc.) should also not trigger exit status updates.
pub(super) fn finish_previous_command(state: &mut ReplState, prompt_kind: PromptKind) {
    if prompt_kind.is_command() && !state.prompt_config.is_shell_enabled() {
        let pending_history_context = std::mem::take(&mut state.pending_history_context);
        let outcome = match pending_history_context {
            PendingHistoryContext::Command { store, history_id } => {
                let outcome = arf_libr::command_outcome();
                if let Some(failed) = outcome_failure_projection(outcome) {
                    record_command_outcome(
                        store,
                        history_id,
                        failed,
                        &state.forget_config,
                        &mut state.sponge_queue,
                    );
                }
                Some(outcome)
            }
            PendingHistoryContext::None => None,
        };

        // Update prompt status indicator for the next prompt
        if let Some(outcome) = outcome {
            match outcome_failure_projection(outcome) {
                Some(failed) => state.prompt_config.set_last_command_failed(failed),
                None => state.prompt_config.set_last_command_outcome_unknown(),
            }
        }

        // Calculate duration for the {duration} prompt placeholder
        state.prompt_config.set_command_duration();

        // Reset error state for the next command
        arf_libr::reset_command_error_state();
    }
}

/// Finalize the lifecycle of a command whose input the formatter rejected.
///
/// A continuation input still belongs to the outer command. It must not
/// finalize that command's history or sponge state merely because formatting
/// this piece failed.
pub(super) fn record_formatter_failure(
    state: &mut ReplState,
    prompt_kind: PromptKind,
    history_handle: &HistoryRuntime,
    save_outcome: Option<HistorySaveOutcome>,
) {
    if formatter_failure_updates_lifecycle(prompt_kind) {
        let history_id = save_outcome.and_then(HistorySaveOutcome::saved_id);
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
}
