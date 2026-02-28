//! Meta command completer for commands starting with `:`.

use super::path::PathCompletionOptions;
use super::string_context::path_to_suggestions;
use crate::external::rig;
use crate::fuzzy::fuzzy_match;
use reedline::{Completer, Span, Suggestion};

/// Definition of a meta command for completion.
struct MetaCommandDef {
    name: &'static str,
    description: &'static str,
    /// Whether this command takes an argument (e.g., `:switch 4.4`).
    /// If true, a trailing space is appended after completion.
    takes_argument: bool,
}

/// Available meta commands.
const META_COMMANDS: &[MetaCommandDef] = &[
    MetaCommandDef {
        name: "help",
        description: "Search R help",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "h",
        description: "Search R help (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "info",
        description: "Show session information",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "session",
        description: "Show session information (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "shell",
        description: "Enter shell mode",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "r",
        description: "Return to R mode",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "system",
        description: "Execute system command",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "reprex",
        description: "Toggle reprex mode",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "autoformat",
        description: "Toggle auto-formatting (requires Air CLI)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "format",
        description: "Toggle auto-formatting (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "commands",
        description: "Show available commands",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "cmds",
        description: "Show available commands (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "restart",
        description: "Restart R session",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "switch",
        description: "Restart with different R version (requires rig)",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "history",
        description: "Manage command history",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "cd",
        description: "Change working directory",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "pushd",
        description: "Push directory and change to it",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "popd",
        description: "Pop directory from stack",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "quit",
        description: "Quit arf",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "exit",
        description: "Quit arf",
        takes_argument: false,
    },
];

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
        if let Some(cmd) = parts.first()
            && (*cmd == "cd" || *cmd == "pushd")
            && (parts.len() > 1 || has_trailing_space)
        {
            let cmd_end = 1 + cmd.len(); // ":" + cmd
            let rest = &trimmed[cmd_end..];
            let ws_after = rest.len() - rest.trim_start().len();
            let arg_start = leading_whitespace + cmd_end + ws_after;
            let partial = if pos > arg_start {
                &line[arg_start..pos]
            } else {
                ""
            };
            return path_to_suggestions(
                partial,
                pos,
                arg_start,
                &PathCompletionOptions {
                    directories_only: true,
                    ..Default::default()
                },
            );
        }

        match (parts.len(), has_trailing_space) {
            (0, _) => {
                // Just ":" - show all commands
                let start = leading_whitespace + 1; // after ':'
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
            (1, false) => {
                // Typing command name, e.g., ":rep" or ":rst" (fuzzy)
                let partial = parts[0];
                let start = leading_whitespace + 1; // after ':'
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
            (1, true) => {
                // Command complete with trailing space - check for subcommands
                let cmd = parts[0];
                if cmd == "switch" {
                    // Complete with R versions from rig
                    self.complete_switch_versions(line, pos, leading_whitespace, "")
                } else if cmd == "history" {
                    // Complete with history subcommands
                    self.complete_history_subcommands(pos, "")
                } else {
                    vec![]
                }
            }
            (2, false) => {
                // Typing subcommand argument
                let cmd = parts[0];
                let partial = parts[1];
                if cmd == "switch" {
                    self.complete_switch_versions(line, pos, leading_whitespace, partial)
                } else if cmd == "history" {
                    self.complete_history_subcommands(pos, partial)
                } else {
                    vec![]
                }
            }
            (2, true) => {
                // Two parts complete with trailing space - check for third level
                let cmd = parts[0];
                let subcmd = parts[1];
                if cmd == "history" && subcmd == "clear" {
                    // Complete with clear targets (r, shell, all)
                    self.complete_history_clear_targets(pos, "")
                } else if cmd == "history" && subcmd == "browse" {
                    // Complete with browse targets (r, shell)
                    self.complete_history_browse_targets(pos, "")
                } else {
                    vec![]
                }
            }
            (3, false) => {
                // Typing third argument
                let cmd = parts[0];
                let subcmd = parts[1];
                let partial = parts[2];
                if cmd == "history" && subcmd == "clear" {
                    self.complete_history_clear_targets(pos, partial)
                } else if cmd == "history" && subcmd == "browse" {
                    self.complete_history_browse_targets(pos, partial)
                } else {
                    vec![]
                }
            }
            _ => {
                // No more completions
                vec![]
            }
        }
    }

    /// Complete R versions for the :switch command.
    fn complete_switch_versions(
        &self,
        line: &str,
        pos: usize,
        _leading_whitespace: usize,
        partial: &str,
    ) -> Vec<Suggestion> {
        // Check if rig is available
        if !rig::rig_available() {
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
    fn complete_history_subcommands(&self, pos: usize, partial: &str) -> Vec<Suggestion> {
        let subcommands = [
            ("browse", "Browse and manage command history"),
            ("clear", "Clear command history"),
            ("schema", "Display database schema and R examples"),
        ];

        let match_len = partial.len();
        subcommands
            .iter()
            .filter(|(name, _)| name.starts_with(partial))
            .map(|(name, desc)| {
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };
                Suggestion {
                    value: name.to_string(),
                    display_override: None,
                    description: Some(desc.to_string()),
                    extra: None,
                    span: Span {
                        start: pos - match_len,
                        end: pos,
                    },
                    append_whitespace: true,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect()
    }

    /// Complete from a list of (name, description) targets.
    fn complete_targets(
        &self,
        pos: usize,
        partial: &str,
        targets: &[(&str, &str)],
    ) -> Vec<Suggestion> {
        let match_len = partial.len();
        targets
            .iter()
            .filter(|(name, _)| name.starts_with(partial))
            .map(|(name, desc)| {
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };
                Suggestion {
                    value: name.to_string(),
                    display_override: None,
                    description: Some(desc.to_string()),
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

    /// Complete history clear targets (r, shell, all).
    fn complete_history_clear_targets(&self, pos: usize, partial: &str) -> Vec<Suggestion> {
        self.complete_targets(
            pos,
            partial,
            &[
                ("r", "Clear R mode history"),
                ("shell", "Clear shell mode history"),
                ("all", "Clear all history"),
            ],
        )
    }

    /// Complete history browse targets (r, shell).
    fn complete_history_browse_targets(&self, pos: usize, partial: &str) -> Vec<Suggestion> {
        self.complete_targets(
            pos,
            partial,
            &[
                ("r", "Browse R mode history"),
                ("shell", "Browse shell mode history"),
            ],
        )
    }
}

impl Default for MetaCommandCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for MetaCommandCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        self.complete_commands(line, pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meta_command_completer_empty_colon() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":", 1);
        assert!(!suggestions.is_empty());
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(values.contains(&"shell"));
        assert!(values.contains(&"r"));
        assert!(values.contains(&"system"));
        assert!(values.contains(&"reprex"));
        assert!(values.contains(&"commands"));
        assert!(values.contains(&"cmds"));
        assert!(values.contains(&"restart"));
        assert!(values.contains(&"switch"));
        assert!(values.contains(&"quit"));
        assert!(values.contains(&"exit"));
    }

    #[test]
    fn test_meta_command_completer_partial_command() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":rep", 4);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "reprex");
    }

    #[test]
    fn test_meta_command_completer_no_subcommands() {
        let mut completer = MetaCommandCompleter::new();
        // All commands have no subcommands
        let suggestions = completer.complete(":reprex ", 8);
        assert!(suggestions.is_empty());
        let suggestions = completer.complete(":commands ", 10);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_meta_command_completer_not_meta_command() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete("print(x)", 8);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_meta_command_completer_has_descriptions() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":", 1);
        let reprex = suggestions.iter().find(|s| s.value == "reprex").unwrap();
        assert!(reprex.description.is_some());
        assert!(reprex.description.as_ref().unwrap().contains("reprex"));
    }

    #[test]
    fn test_meta_command_completer_excludes_r_command() {
        // In R mode, `:r` should be excluded from completion
        let mut completer = MetaCommandCompleter::with_exclusions(vec!["r"]);
        let suggestions = completer.complete(":", 1);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(!values.contains(&"r"), "`:r` should be excluded in R mode");
        assert!(
            values.contains(&"shell"),
            "`:shell` should still be present"
        );
    }

    #[test]
    fn test_meta_command_completer_excludes_shell_mode_commands() {
        // In Shell mode, R-specific commands should be excluded from completion
        let mut completer = MetaCommandCompleter::with_exclusions(vec![
            "shell",
            "system",
            "autoformat",
            "format",
            "restart",
            "reprex",
            "switch",
            "h",
            "help",
        ]);
        let suggestions = completer.complete(":", 1);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();

        // These should be excluded in Shell mode
        assert!(!values.contains(&"shell"), "`:shell` should be excluded");
        assert!(!values.contains(&"system"), "`:system` should be excluded");
        assert!(
            !values.contains(&"autoformat"),
            "`:autoformat` should be excluded"
        );
        assert!(!values.contains(&"format"), "`:format` should be excluded");
        assert!(
            !values.contains(&"restart"),
            "`:restart` should be excluded"
        );
        assert!(!values.contains(&"reprex"), "`:reprex` should be excluded");
        assert!(!values.contains(&"switch"), "`:switch` should be excluded");
        assert!(!values.contains(&"h"), "`:h` should be excluded");
        assert!(!values.contains(&"help"), "`:help` should be excluded");

        // These should still be present in Shell mode
        assert!(values.contains(&"r"), "`:r` should be present");
        assert!(
            values.contains(&"commands"),
            "`:commands` should be present"
        );
        assert!(values.contains(&"quit"), "`:quit` should be present");
        assert!(values.contains(&"exit"), "`:exit` should be present");
    }

    #[test]
    fn test_meta_command_completer_exclusion_affects_partial_match() {
        // Even with partial match, excluded commands should not appear
        let mut completer = MetaCommandCompleter::with_exclusions(vec!["r"]);
        // Typing ":r" should not match excluded "r" command
        let suggestions = completer.complete(":r", 2);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            !values.contains(&"r"),
            "`:r` should not appear even with partial match"
        );
        // But "restart" and "reprex" should still appear
        assert!(values.contains(&"restart"));
        assert!(values.contains(&"reprex"));
    }

    #[test]
    fn test_meta_command_append_whitespace_for_commands_with_arguments() {
        // Commands that take arguments should have append_whitespace: true
        let mut completer = MetaCommandCompleter::new();

        // :switch takes an argument (R version)
        let suggestions = completer.complete(":sw", 3);
        let switch = suggestions.iter().find(|s| s.value == "switch").unwrap();
        assert!(
            switch.append_whitespace,
            "`:switch` should append whitespace because it takes an argument"
        );

        // :system takes an argument (shell command)
        let suggestions = completer.complete(":sys", 4);
        let system = suggestions.iter().find(|s| s.value == "system").unwrap();
        assert!(
            system.append_whitespace,
            "`:system` should append whitespace because it takes an argument"
        );
    }

    #[test]
    fn test_meta_command_no_append_whitespace_for_commands_without_arguments() {
        // Commands that don't take arguments should have append_whitespace: false
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":", 1);

        // Commands without arguments
        for cmd_name in &[
            "shell",
            "r",
            "reprex",
            "autoformat",
            "format",
            "commands",
            "cmds",
            "restart",
            "quit",
            "exit",
        ] {
            if let Some(cmd) = suggestions.iter().find(|s| s.value == *cmd_name) {
                assert!(
                    !cmd.append_whitespace,
                    "`:{}` should NOT append whitespace because it takes no argument",
                    cmd_name
                );
            }
        }
    }

    #[test]
    fn test_meta_command_match_indices_for_partial_command() {
        // When typing a partial command, match_indices should highlight the matched prefix
        let mut completer = MetaCommandCompleter::new();

        // Typing ":rep" should match "reprex" and highlight positions 0,1,2
        let suggestions = completer.complete(":rep", 4);
        assert_eq!(suggestions.len(), 1);
        let reprex = &suggestions[0];
        assert_eq!(reprex.value, "reprex");
        assert_eq!(reprex.match_indices, Some(vec![0, 1, 2]));
    }

    #[test]
    fn test_meta_command_match_indices_none_for_empty_input() {
        // When just ":" is typed, no prefix to highlight
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":", 1);
        assert!(!suggestions.is_empty());
        for suggestion in &suggestions {
            assert_eq!(
                suggestion.match_indices, None,
                "`:` with no partial input should have match_indices: None"
            );
        }
    }

    #[test]
    fn test_meta_command_match_indices_single_char() {
        // Single character fuzzy match - 'r' matches at different positions in different commands
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":r", 2);
        // With fuzzy matching, `:r` matches any command containing 'r':
        // - "r" at position 0
        // - "reprex" at position 0
        // - "restart" at position 0
        // - "autoformat" at position 6
        // - "format" at position 2
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(values.contains(&"r"));
        assert!(values.contains(&"reprex"));
        assert!(values.contains(&"restart"));
        assert!(values.contains(&"autoformat"));
        assert!(values.contains(&"format"));

        // Verify match_indices are correct for each
        for suggestion in &suggestions {
            let expected_pos = suggestion
                .value
                .find('r')
                .or_else(|| suggestion.value.find('R'));
            assert_eq!(
                suggestion.match_indices,
                expected_pos.map(|p| vec![p]),
                "`:r` should highlight first 'r' position in `{}`",
                suggestion.value
            );
        }
    }

    // --- Fuzzy matching integration tests ---
    // Note: Direct fuzzy_match tests are in fuzzy.rs module

    #[test]
    fn test_meta_command_fuzzy_matching() {
        let mut completer = MetaCommandCompleter::new();

        // ":rst" should fuzzy match "restart"
        let suggestions = completer.complete(":rst", 4);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            values.contains(&"restart"),
            "`:rst` should fuzzy match `restart`, got: {:?}",
            values
        );

        // Check match_indices for fuzzy match
        let restart = suggestions.iter().find(|s| s.value == "restart").unwrap();
        assert_eq!(
            restart.match_indices,
            Some(vec![0, 2, 3]),
            "`:rst` should highlight positions 0, 2, 3 in `restart`"
        );
    }

    #[test]
    fn test_meta_command_fuzzy_matching_af_autoformat() {
        let mut completer = MetaCommandCompleter::new();

        // ":af" should fuzzy match "autoformat"
        let suggestions = completer.complete(":af", 3);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            values.contains(&"autoformat"),
            "`:af` should fuzzy match `autoformat`, got: {:?}",
            values
        );

        // Check match_indices
        let autoformat = suggestions
            .iter()
            .find(|s| s.value == "autoformat")
            .unwrap();
        assert_eq!(
            autoformat.match_indices,
            Some(vec![0, 4]),
            "`:af` should highlight positions 0, 4 in `autoformat`"
        );
    }

    #[test]
    fn test_meta_command_fuzzy_matching_cms_cmds() {
        let mut completer = MetaCommandCompleter::new();

        // ":cms" should match "cmds" - c=0, m=1, d=2, s=3
        let suggestions = completer.complete(":cms", 4);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(values.contains(&"cmds"), "`:cms` should fuzzy match `cmds`");

        let cmds = suggestions.iter().find(|s| s.value == "cmds").unwrap();
        assert_eq!(
            cmds.match_indices,
            Some(vec![0, 1, 3]),
            "`:cms` should highlight positions 0, 1, 3 in `cmds`"
        );
    }

    #[test]
    fn test_meta_command_fuzzy_no_match() {
        let mut completer = MetaCommandCompleter::new();

        // ":xyz" should not match any command
        let suggestions = completer.complete(":xyz", 4);
        assert!(
            suggestions.is_empty(),
            "`:xyz` should not match any command"
        );
    }

    #[test]
    fn test_meta_command_help_no_append_whitespace() {
        // :help and :h should NOT append whitespace after completion
        // because they open an interactive help browser (no argument needed)
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":hel", 4);
        let help = suggestions.iter().find(|s| s.value == "help").unwrap();
        assert!(
            !help.append_whitespace,
            "`:help` should NOT append whitespace"
        );

        let suggestions = completer.complete(":h", 2);
        let h = suggestions.iter().find(|s| s.value == "h").unwrap();
        assert!(!h.append_whitespace, "`:h` should NOT append whitespace");
    }

    // --- cd/pushd/popd completion tests ---

    #[test]
    fn test_meta_cd_completes_directories() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();
        std::fs::File::create(tmp.path().join("file.txt")).unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":cd ", 4);

        // Should only contain directories (directories_only: true)
        assert!(!suggestions.is_empty(), "Should have directory completions");
        for s in &suggestions {
            assert!(
                s.value.ends_with('/'),
                "cd completion should only show directories, got: {}",
                s.value
            );
        }
        assert!(
            suggestions.iter().any(|s| s.value == "subdir/"),
            "Should contain subdir/"
        );
        assert!(
            !suggestions.iter().any(|s| s.value == "file.txt"),
            "Should not contain files"
        );
    }

    #[test]
    fn test_meta_cd_partial_path() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":cd su", 6);

        assert!(
            suggestions.iter().any(|s| s.value == "subdir/"),
            "Should fuzzy-match subdir/, got: {:?}",
            suggestions.iter().map(|s| &s.value).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_meta_pushd_completes_directories() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("mydir")).unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":pushd ", 7);

        assert!(
            suggestions.iter().any(|s| s.value == "mydir/"),
            "Should contain mydir/"
        );
    }

    #[test]
    fn test_meta_popd_no_completion() {
        // ":popd " should NOT show path completions (takes no argument)
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":popd ", 6);
        assert!(
            suggestions.is_empty(),
            "popd should not have completions, got: {:?}",
            suggestions.iter().map(|s| &s.value).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_meta_cd_nested_path() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(src.join("inner")).unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":cd src/", 8);

        assert!(
            suggestions.iter().any(|s| s.value == "src/inner/"),
            "Should list src/inner/, got: {:?}",
            suggestions.iter().map(|s| &s.value).collect::<Vec<_>>()
        );
    }
}
