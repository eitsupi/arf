//! Completion of meta command names and of the path argument of `:cd`/`:pushd`.

use super::MetaCommandCompleter;
use crate::completion::path::{PathCompletionOptions, path_to_suggestions};
use crate::fuzzy::fuzzy_match;
use crate::meta_command_catalog::{META_COMMANDS, MetaCommand, lookup_meta_command};
use reedline::{Span, Suggestion};

impl MetaCommandCompleter {
    /// Complete the directory argument of `:cd` and `:pushd`.
    ///
    /// Returns `None` when the line is not a `:cd`/`:pushd` argument position.
    /// The argument is taken as the raw substring after the command, so paths
    /// containing spaces work.
    pub(super) fn complete_directory_argument(
        line: &str,
        pos: usize,
        leading_whitespace: usize,
        parts: &[&str],
        has_trailing_space: bool,
    ) -> Option<Vec<Suggestion>> {
        let cmd = parts.first()?;
        let is_directory_command = matches!(
            lookup_meta_command(cmd).map(|def| def.command),
            Some(MetaCommand::Cd | MetaCommand::Pushd)
        );
        if !is_directory_command || !(parts.len() > 1 || has_trailing_space) {
            return None;
        }

        let trimmed = line.trim_start();
        let cmd_end = 1 + cmd.len(); // ":" + cmd
        let rest = &trimmed[cmd_end..];
        let ws_after = rest.len() - rest.trim_start().len();
        let arg_start = leading_whitespace + cmd_end + ws_after;
        let partial = if pos > arg_start {
            &line[arg_start..pos]
        } else {
            ""
        };
        Some(path_to_suggestions(
            partial,
            pos,
            arg_start,
            &PathCompletionOptions {
                directories_only: true,
                ..Default::default()
            },
        ))
    }

    /// Complete with every available command (just `:` was typed).
    pub(super) fn complete_all_commands(&self, start: usize, pos: usize) -> Vec<Suggestion> {
        let mut suggestions: Vec<Suggestion> = META_COMMANDS
            .iter()
            .filter(|cmd| !self.excluded_commands.contains(&cmd.name))
            .map(|cmd| Suggestion {
                value: cmd.name.to_string(),
                display_override: None,
                description: Some(cmd.description.to_string()),
                extra: None,
                span: Span { start, end: pos },
                append_whitespace: cmd.takes_argument,
                style: None,
                match_indices: None,
            })
            .collect();
        // Sort by length so shorter aliases (h, r, cmds) appear before longer forms
        suggestions.sort_by_key(|s| s.value.len());
        suggestions
    }

    /// Complete a partially typed command name with fuzzy matching.
    pub(super) fn complete_command_name(
        &self,
        partial: &str,
        start: usize,
        pos: usize,
    ) -> Vec<Suggestion> {
        let mut suggestions: Vec<Suggestion> = META_COMMANDS
            .iter()
            .filter(|cmd| !self.excluded_commands.contains(&cmd.name))
            .filter_map(|cmd| {
                fuzzy_match(partial, cmd.name).map(|m| Suggestion {
                    value: cmd.name.to_string(),
                    display_override: None,
                    description: Some(cmd.description.to_string()),
                    extra: None,
                    span: Span { start, end: pos },
                    append_whitespace: cmd.takes_argument,
                    style: None,
                    match_indices: if m.indices.is_empty() {
                        None
                    } else {
                        Some(m.indices)
                    },
                })
            })
            .collect();
        // Sort by length so shorter aliases (h, r, cmds) appear before longer forms
        suggestions.sort_by_key(|s| s.value.len());
        suggestions
    }
}
