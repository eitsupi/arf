//! Completion of meta command subcommands and argument values.

use super::MetaCommandCompleter;
use crate::external::rig;
use crate::meta_command_catalog::{ArgumentChoice, HISTORY_SUBCOMMANDS, IPC_SUBCOMMANDS};
use reedline::{Span, Suggestion};

impl MetaCommandCompleter {
    /// Complete R versions for the :switch command.
    pub(super) fn complete_switch_versions(
        &self,
        line: &str,
        pos: usize,
        partial: &str,
    ) -> Vec<Suggestion> {
        // Check if rig is available
        if rig::rig_available().is_err() {
            return vec![];
        }

        // Get installed R versions from rig
        let versions = match rig::list_versions() {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        // Calculate span start (after ":switch ")
        let start = if partial.is_empty() {
            pos
        } else {
            // Find where the partial version starts
            line.rfind(partial).unwrap_or(pos)
        };

        // Build suggestions from versions
        let match_len = partial.len();
        let mut suggestions: Vec<Suggestion> = versions
            .iter()
            .filter(|v| v.name.starts_with(partial) || v.version.starts_with(partial))
            .map(|v| {
                let description = if v.default {
                    format!("R {} (default)", v.version)
                } else {
                    format!("R {}", v.version)
                };
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };
                Suggestion {
                    value: v.name.clone(),
                    display_override: None,
                    description: Some(description),
                    extra: None,
                    span: Span { start, end: pos },
                    append_whitespace: false,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect();

        // Also add aliases as suggestions
        for v in &versions {
            for alias in &v.aliases {
                if alias.starts_with(partial) {
                    let indices = if match_len > 0 {
                        Some((0..match_len).collect())
                    } else {
                        None
                    };
                    suggestions.push(Suggestion {
                        value: alias.clone(),
                        display_override: None,
                        description: Some(format!("R {} (alias)", v.version)),
                        extra: None,
                        span: Span { start, end: pos },
                        append_whitespace: false,
                        style: None,
                        match_indices: indices,
                    });
                }
            }
        }

        suggestions
    }
    /// Complete history subcommands (browse, clear, schema).
    pub(super) fn complete_history_subcommands(
        &self,
        pos: usize,
        partial: &str,
    ) -> Vec<Suggestion> {
        let mut suggestions = self.complete_targets(pos, partial, HISTORY_SUBCOMMANDS);
        for suggestion in &mut suggestions {
            suggestion.append_whitespace = true;
        }
        suggestions
    }

    /// Complete IPC subcommands (start, stop, status, send-policy).
    pub(super) fn complete_ipc_subcommands(&self, pos: usize, partial: &str) -> Vec<Suggestion> {
        let mut suggestions = self.complete_targets(pos, partial, IPC_SUBCOMMANDS);
        for suggestion in &mut suggestions {
            suggestion.append_whitespace = suggestion.value == "send-policy";
        }
        suggestions
    }

    /// Complete from a list of named choices.
    pub(super) fn complete_targets(
        &self,
        pos: usize,
        partial: &str,
        targets: &[ArgumentChoice],
    ) -> Vec<Suggestion> {
        let match_len = partial.len();
        targets
            .iter()
            .filter(|target| target.name.starts_with(partial))
            .map(|target| {
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };
                Suggestion {
                    value: target.name.to_string(),
                    display_override: None,
                    description: Some(target.description.to_string()),
                    extra: None,
                    span: Span {
                        start: pos - match_len,
                        end: pos,
                    },
                    append_whitespace: false,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect()
    }
}
