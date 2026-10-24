//! Meta command processing.

mod commands;
mod directory;
mod dispatch;
mod history;
mod ipc;

use crate::config::RSourceStatus;
use crate::history::{HistoryRuntime, HistoryStore};
use crate::meta_command_catalog::{MetaCommand, lookup_meta_command};
use crate::pager::HistoryDbMode;
use std::path::{Path, PathBuf};

use super::arf_println;
use super::reprex::ReprexRuntime;
use super::state::PromptRuntimeConfig;

pub(super) use directory::dir_command_hint;
pub(super) use dispatch::{MetaAction, SessionInfoContext, handle_meta_command_result};

/// Result of processing a meta command.
pub(super) enum MetaCommandResult {
    /// Command was handled, continue with new prompt
    Handled,
    /// User wants to exit
    Exit,
    /// Unknown command
    Unknown(String),
    /// Shell command was executed inline (for :system)
    ShellExecuted,
    /// Restart the process with optional R version
    Restart(Option<String>),
    /// Open the help browser with the given query (caller runs pager)
    ShowHelpBrowser(String),
    /// Display session info (caller runs pager)
    ShowSessionInfo,
    /// Display changelog (caller runs pager)
    ShowChangelog,
    /// Open the history browser (caller runs pager)
    ShowHistoryBrowser {
        store: HistoryStore,
        mode: HistoryDbMode,
    },
    /// Clear stores after the caller has finalized the command provenance.
    ClearHistory {
        stores: Vec<(&'static str, HistoryStore)>,
    },
    /// Display history schema (caller runs pager)
    ShowHistorySchema,
}

/// Session state that meta commands read or change.
pub(super) struct MetaCommandContext<'a> {
    pub(super) prompt_config: &'a mut PromptRuntimeConfig,
    pub(super) reprex: &'a mut ReprexRuntime,
    pub(super) r_history: &'a HistoryRuntime,
    pub(super) shell_history: &'a HistoryRuntime,
    pub(super) r_source_status: &'a RSourceStatus,
    pub(super) dir_stack: &'a mut Vec<PathBuf>,
    pub(super) history_session_id: Option<i64>,
    pub(super) r_home: Option<&'a Path>,
}

/// Process a meta command (starting with `:`) and return the result.
pub(super) fn process_meta_command(
    input: &str,
    ctx: &mut MetaCommandContext<'_>,
) -> Option<MetaCommandResult> {
    process_meta_command_with_ipc_expose(input, ctx, false)
}

/// Process a meta command from a verified interactive R prompt.
///
/// Exposing handlers can evaluate R code, so callers outside the ReadConsole
/// command prompt must leave this capability disabled.
pub(super) fn process_meta_command_with_ipc_expose(
    input: &str,
    ctx: &mut MetaCommandContext<'_>,
    allow_ipc_expose: bool,
) -> Option<MetaCommandResult> {
    let trimmed = input.trim();
    if !trimmed.starts_with(':') {
        return None;
    }

    let after_colon = &trimmed[1..];
    let parts: Vec<&str> = after_colon.split_whitespace().collect();
    let cmd = parts.first().copied().unwrap_or("");

    if cmd.is_empty() {
        // Just ":" with nothing after - show help hint
        arf_println!("Type :commands for available commands");
        return Some(MetaCommandResult::Handled);
    }
    let Some(definition) = lookup_meta_command(cmd) else {
        return Some(MetaCommandResult::Unknown(cmd.to_string()));
    };

    Some(match definition.command {
        MetaCommand::Reprex => commands::handle_reprex(ctx, &parts),
        MetaCommand::Shell => commands::handle_shell(ctx),
        MetaCommand::R => commands::handle_r(ctx),
        MetaCommand::System => commands::handle_system(after_colon),
        MetaCommand::Restart => commands::handle_restart(cmd),
        MetaCommand::Switch => commands::handle_switch(ctx, cmd, after_colon),
        MetaCommand::History => history::handle_history(ctx, &parts),
        MetaCommand::Help => commands::handle_help(&parts),
        MetaCommand::Info => MetaCommandResult::ShowSessionInfo,
        MetaCommand::Changelog => MetaCommandResult::ShowChangelog,
        MetaCommand::Cd => commands::handle_cd(after_colon),
        MetaCommand::Pushd => commands::handle_pushd(ctx, after_colon),
        MetaCommand::Popd => commands::handle_popd(ctx),
        MetaCommand::Ipc => ipc::handle_ipc(ctx, &parts, input, allow_ipc_expose),
        MetaCommand::Commands => commands::handle_commands(),
        MetaCommand::Quit => MetaCommandResult::Exit,
    })
}

#[cfg(test)]
mod tests;
