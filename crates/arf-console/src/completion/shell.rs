//! Shell mode completion.
//!
//! Provides path completion and optional executable name completion
//! for shell mode commands.

use super::meta::MetaCommandCompleter;
use super::path::PathCompletionOptions;
use super::string_context::path_to_suggestions;
use reedline::{Completer, Span, Suggestion};

/// Shell command separators that start a new token and command context.
const SEPARATORS: &[char] = &['|', ';', '&', '<', '>'];

/// Find the byte position where the current token starts.
///
/// Walks forward through the line up to `pos`, tracking the position after
/// the last whitespace or shell separator character.
fn current_token_start(line: &str, pos: usize) -> usize {
    let slice = &line[..pos];
    let mut last_sep_end = 0;
    for (byte_pos, ch) in slice.char_indices() {
        if ch.is_whitespace() || SEPARATORS.contains(&ch) {
            last_sep_end = byte_pos + ch.len_utf8();
        }
    }
    last_sep_end
}

/// Determine whether the cursor is in a command-name position.
///
/// Returns true if the current token is the first non-whitespace token in the
/// current command segment (i.e., after the last shell separator like `|`, `;`).
fn is_command_position(line: &str, pos: usize) -> bool {
    let token_start = current_token_start(line, pos);
    let before_token = &line[..token_start];

    // Find the byte position after the last shell segment separator
    let last_sep_end = {
        let mut sep_end = 0;
        for (byte_pos, ch) in before_token.char_indices() {
            if SEPARATORS.contains(&ch) {
                sep_end = byte_pos + ch.len_utf8();
            }
        }
        sep_end
    };

    // If everything between the last separator and token start is whitespace,
    // the current token is the command name in the segment
    before_token[last_sep_end..]
        .chars()
        .all(|c| c.is_whitespace())
}

/// Collect all names of files found in PATH directories.
///
/// Results are sorted and deduplicated. No executable-bit filtering is applied
/// (false positives are acceptable for completion purposes).
fn collect_path_executables() -> Vec<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for dir in std::env::split_paths(&path_var) {
        if let Ok(read_dir) = std::fs::read_dir(&dir) {
            for entry in read_dir.filter_map(|e| e.ok()) {
                names.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }

    let mut result: Vec<String> = names.into_iter().collect();
    result.sort();
    result
}

/// Completer for shell mode.
///
/// Provides path completion for all arguments and optionally completes
/// executable names from PATH when the cursor is in command position.
pub struct ShellCompleter {
    meta_completer: MetaCommandCompleter,
    command_names: bool,
    command_cache: Option<Vec<String>>,
}

impl ShellCompleter {
    /// Create a new `ShellCompleter`.
    ///
    /// When `command_names` is true, Tab-completing the first token of a shell
    /// command also suggests executable names from PATH directories.
    pub fn new(command_names: bool) -> Self {
        Self {
            meta_completer: MetaCommandCompleter::with_exclusions(vec![
                "shell",
                "system",
                "autoformat",
                "format",
                "restart",
                "reprex",
                "switch",
                "h",
                "help",
            ]),
            command_names,
            command_cache: None,
        }
    }

    fn get_command_names(&mut self) -> &[String] {
        if self.command_cache.is_none() {
            self.command_cache = Some(collect_path_executables());
        }
        self.command_cache.as_ref().unwrap()
    }
}

impl Completer for ShellCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let trimmed = line.trim_start();

        // Delegate meta commands to MetaCommandCompleter
        if trimmed.starts_with(':') {
            return self.meta_completer.complete(line, pos);
        }

        // Find the current token start and extract the partial text
        let token_start = current_token_start(line, pos);
        let partial = &line[token_start..pos];

        // Get path completions for the current token
        let mut suggestions =
            path_to_suggestions(partial, pos, token_start, &PathCompletionOptions::default());

        // Wrap paths containing spaces in double quotes for shell safety
        for s in &mut suggestions {
            if s.value.contains(' ') {
                s.value = format!("\"{}\"", s.value);
            }
        }

        // Add command name completions when in command position and enabled
        if self.command_names && is_command_position(line, pos) {
            let cmd_suggestions: Vec<Suggestion> = self
                .get_command_names()
                .iter()
                .filter(|name| partial.is_empty() || name.starts_with(partial))
                .map(|name| Suggestion {
                    value: name.clone(),
                    display_override: None,
                    description: Some("command".to_string()),
                    extra: None,
                    span: Span {
                        start: token_start,
                        end: pos,
                    },
                    append_whitespace: false,
                    style: None,
                    match_indices: None,
                })
                .collect();

            // Command names follow path suggestions
            suggestions.extend(cmd_suggestions);
        }

        suggestions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_token_start_first_token() {
        assert_eq!(current_token_start("ls", 2), 0);
        assert_eq!(current_token_start("  ls", 4), 2);
    }

    #[test]
    fn test_current_token_start_argument() {
        // "ls /tmp/" - cursor at end, token starts after space
        assert_eq!(current_token_start("ls /tmp/", 8), 3);
    }

    #[test]
    fn test_current_token_start_after_pipe() {
        // "ls | cat" - cursor at 8, token starts at 5
        assert_eq!(current_token_start("ls | cat", 8), 5);
    }

    #[test]
    fn test_current_token_start_empty() {
        assert_eq!(current_token_start("", 0), 0);
    }

    #[test]
    fn test_is_command_position_first_token() {
        assert!(is_command_position("ls", 2));
        assert!(is_command_position("  ls", 4));
        assert!(is_command_position("", 0));
    }

    #[test]
    fn test_is_command_position_argument() {
        assert!(!is_command_position("ls /tmp", 7));
        assert!(!is_command_position("cat foo bar", 11));
    }

    #[test]
    fn test_is_command_position_after_pipe() {
        assert!(is_command_position("ls | ", 5));
        assert!(is_command_position("ls | cat", 8));
    }

    #[test]
    fn test_is_command_position_after_semicolon() {
        assert!(is_command_position("echo hi; ", 9));
        assert!(is_command_position("echo hi; ls", 11));
    }

    #[test]
    fn test_shell_completer_delegates_meta_commands() {
        let mut completer = ShellCompleter::new(false);
        // :cd is available in shell mode (not excluded)
        let suggestions = completer.complete(":c", 2);
        assert!(
            suggestions.iter().any(|s| s.value == "cd"),
            "should suggest :cd for :c"
        );
    }

    #[test]
    fn test_shell_completer_excludes_shell_mode_meta_commands() {
        let mut completer = ShellCompleter::new(false);
        let suggestions = completer.complete(":", 1);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            !values.contains(&"shell"),
            ":shell should be excluded in shell mode"
        );
        assert!(
            !values.contains(&"help"),
            ":help should be excluded in shell mode"
        );
    }

    #[test]
    fn test_shell_completer_no_command_names_when_disabled() {
        let mut completer = ShellCompleter::new(false);
        let suggestions = completer.complete("", 0);
        assert!(
            suggestions
                .iter()
                .all(|s| s.description.as_deref() != Some("command")),
            "command_names=false should not produce command suggestions"
        );
    }

    #[test]
    fn test_shell_completer_path_completion_for_argument() {
        // Path completion should work for arguments even when command_names=false
        let mut completer = ShellCompleter::new(false);
        // Completing after "cat " - any path suggestions should have span starting at 4
        let suggestions = completer.complete("cat /", 5);
        for s in &suggestions {
            assert_eq!(s.span.start, 4, "span should start at the token start (4)");
        }
    }
}
