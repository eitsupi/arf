//! Execution of the side effects requested by a processed meta command.

use super::MetaCommandResult;
use crate::config::{ConfigFileInfo, RSourceStatus, ResolvedHistoryLocation};
use crate::history::HistoryRuntime;
use crate::repl::arf_println;
use crate::repl::pager_ui::{
    run_pager_help_browser, run_pager_history_browser, with_ipc_alternate_guard,
};
use crate::repl::reprex::ReprexRuntime;
use crate::repl::shell::restart_process;
use crate::repl::state::PromptRuntimeConfig;

/// Result of handling a meta command in the REPL loop.
pub(in crate::repl) enum MetaAction {
    /// Continue the REPL loop (show next prompt).
    Continue,
    /// The user requested exit.
    Exit,
}

/// Context for displaying session info in the pager.
pub(in crate::repl) struct SessionInfoContext<'a> {
    pub(in crate::repl) prompt_config: &'a PromptRuntimeConfig,
    pub(in crate::repl) reprex: &'a ReprexRuntime,
    pub(in crate::repl) config_file_info: &'a ConfigFileInfo,
    pub(in crate::repl) history_location: &'a ResolvedHistoryLocation,
    pub(in crate::repl) r_history: &'a HistoryRuntime,
    pub(in crate::repl) shell_history: &'a HistoryRuntime,
    pub(in crate::repl) r_source_status: &'a RSourceStatus,
}

/// Handle a `MetaCommandResult`, executing pager side effects as needed.
///
/// Returns `MetaAction::Exit` if the user wants to quit, otherwise `MetaAction::Continue`.
/// This is the single place where all `MetaCommandResult` variants are dispatched,
/// shared by both `Repl::run` (pre-R-init loop) and `read_console_callback` (main REPL).
pub(in crate::repl) fn handle_meta_command_result(
    result: MetaCommandResult,
    ctx: &SessionInfoContext<'_>,
) -> MetaAction {
    match result {
        MetaCommandResult::Handled | MetaCommandResult::ShellExecuted => MetaAction::Continue,
        MetaCommandResult::Exit => MetaAction::Exit,
        MetaCommandResult::Unknown(cmd) => {
            arf_println!(
                "Unknown command: {}. Type :commands for available commands.",
                cmd
            );
            MetaAction::Continue
        }
        MetaCommandResult::Restart(version) => {
            restart_process(version.as_deref());
            MetaAction::Continue
        }
        MetaCommandResult::ShowHelpBrowser(query) => {
            run_pager_help_browser(&query);
            MetaAction::Continue
        }
        MetaCommandResult::ShowSessionInfo => {
            with_ipc_alternate_guard(|| {
                crate::pager::display_session_info(
                    ctx.prompt_config,
                    ctx.reprex,
                    ctx.config_file_info,
                    ctx.r_history,
                    ctx.shell_history,
                    ctx.r_source_status,
                );
            });
            MetaAction::Continue
        }
        MetaCommandResult::ShowChangelog => {
            with_ipc_alternate_guard(crate::pager::display_changelog);
            MetaAction::Continue
        }
        MetaCommandResult::ShowHistoryBrowser { store, mode } => {
            run_pager_history_browser(&store, mode);
            MetaAction::Continue
        }
        MetaCommandResult::ClearHistory { stores } => {
            let mut cleared_count = 0i64;
            for (name, store) in stores {
                match store.count_all() {
                    Ok(count) if count > 0 => match store.clear() {
                        Ok(()) => cleared_count += count,
                        Err(error) => arf_println!("Failed to clear {} history: {}", name, error),
                    },
                    Ok(_) => {}
                    Err(error) => arf_println!("Failed to read {} history: {}", name, error),
                }
            }
            arf_println!("Cleared {} history entries.", cleared_count);
            MetaAction::Continue
        }
        MetaCommandResult::ShowHistorySchema => {
            if let Err(e) = with_ipc_alternate_guard(|| {
                crate::pager::history_schema::show_schema_pager(ctx.history_location)
            }) {
                arf_println!("Error: {}", e);
            }
            MetaAction::Continue
        }
    }
}
