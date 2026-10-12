//! Meta command completer for commands starting with `:`.

mod commands;
mod subcommands;

use crate::meta_command_catalog::{
    HISTORY_BROWSE_TARGETS, HISTORY_CLEAR_TARGETS, IPC_SEND_POLICIES, MetaCommand, REPREX_MODES,
    lookup_meta_command,
};
use reedline::{Completer, CompletionResult, Suggestion};

/// Completer for meta commands (starting with `:`).
pub struct MetaCommandCompleter {
    /// Commands to exclude from completion (e.g., `:r` in R mode, `:shell` in Shell mode).
    excluded_commands: Vec<&'static str>,
}

impl MetaCommandCompleter {
    pub fn new() -> Self {
        MetaCommandCompleter {
            excluded_commands: vec![],
        }
    }

    /// Create a new completer with specified commands excluded from completion.
    pub fn with_exclusions(excluded_commands: Vec<&'static str>) -> Self {
        MetaCommandCompleter { excluded_commands }
    }

    /// Commands excluded from meta completion while in shell mode.
    pub(crate) fn shell_mode_exclusions() -> Vec<&'static str> {
        vec![
            "shell", "system", "restart", "reprex", "switch", "h", "help",
        ]
    }

    /// Complete meta commands.
    fn complete_commands(&self, line: &str, pos: usize) -> Vec<Suggestion> {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(':') {
            return vec![];
        }

        // Get the part after ':'
        let after_colon = &trimmed[1..];

        // Check if there's a trailing space (user finished typing and wants subcommands)
        let has_trailing_space = after_colon.ends_with(' ') || after_colon.ends_with('\t');
        let parts: Vec<&str> = after_colon.split_whitespace().collect();

        // Calculate the start position for the span
        let leading_whitespace = line.len() - trimmed.len();

        // Handle path completion for :cd and :pushd
        // This is done before the main match so that paths with spaces work correctly
        // (we extract the raw substring instead of using split_whitespace parts).
        if let Some(suggestions) = Self::complete_directory_argument(
            line,
            pos,
            leading_whitespace,
            &parts,
            has_trailing_space,
        ) {
            return suggestions;
        }

        match (parts.len(), has_trailing_space) {
            // Just ":" - show all commands
            (0, _) => self.complete_all_commands(leading_whitespace + 1, pos),
            // Typing command name, e.g., ":rep" or ":rst" (fuzzy)
            (1, false) => self.complete_command_name(parts[0], leading_whitespace + 1, pos),
            // Command complete with trailing space - check for subcommands
            (1, true) => self.complete_first_argument(line, pos, parts[0], ""),
            // Typing subcommand argument
            (2, false) => self.complete_first_argument(line, pos, parts[0], parts[1]),
            // Two parts complete with trailing space - check for third level
            (2, true) => self.complete_second_argument(pos, parts[0], parts[1], ""),
            // Typing third argument
            (3, false) => self.complete_second_argument(pos, parts[0], parts[1], parts[2]),
            // No more completions
            _ => vec![],
        }
    }

    /// Complete the first argument of a command (its subcommand or value).
    fn complete_first_argument(
        &self,
        line: &str,
        pos: usize,
        cmd: &str,
        partial: &str,
    ) -> Vec<Suggestion> {
        // `:switch!` is a distinct spelling of `:switch` and gets the same
        // version completions.
        match lookup_meta_command(cmd).map(|def| def.command) {
            Some(MetaCommand::Switch) => self.complete_switch_versions(line, pos, partial),
            Some(MetaCommand::History) => self.complete_history_subcommands(pos, partial),
            Some(MetaCommand::Ipc) => self.complete_ipc_subcommands(pos, partial),
            Some(MetaCommand::Reprex) => self.complete_targets(pos, partial, REPREX_MODES),
            _ => vec![],
        }
    }

    /// Complete the second argument of a command (a target of its subcommand).
    fn complete_second_argument(
        &self,
        pos: usize,
        cmd: &str,
        subcmd: &str,
        partial: &str,
    ) -> Vec<Suggestion> {
        match (lookup_meta_command(cmd).map(|def| def.command), subcmd) {
            (Some(MetaCommand::History), "clear") => {
                self.complete_targets(pos, partial, HISTORY_CLEAR_TARGETS)
            }
            (Some(MetaCommand::History), "browse") => {
                self.complete_targets(pos, partial, HISTORY_BROWSE_TARGETS)
            }
            (Some(MetaCommand::Ipc), "send-policy") => {
                self.complete_targets(pos, partial, IPC_SEND_POLICIES)
            }
            _ => vec![],
        }
    }
}

impl Default for MetaCommandCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for MetaCommandCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        CompletionResult::fresh(self.complete_commands(line, pos))
    }
}

#[cfg(test)]
mod tests;
