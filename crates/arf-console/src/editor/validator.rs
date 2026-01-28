//! R expression validator using tree-sitter.

use crate::editor::mode::EditorStateRef;
use reedline::{ValidationResult, Validator};

/// Validator for R expressions using tree-sitter.
/// Checks if the input is a complete R expression or needs more input.
///
/// # Environment Variables
///
/// - `R_TERM_VALIDATOR_DEBUG`: Set to `1` to enable debug logging.
///   Logs are written to `arf-validator.log` in the system temp directory.
///   Useful for diagnosing validation issues. Default: disabled.
pub struct RValidator {
    /// Optional reference to editor state for synchronization.
    /// When set, the validator will update the shadow state with the actual
    /// buffer content, keeping it in sync with reedline's internal state.
    editor_state: Option<EditorStateRef>,
}

impl RValidator {
    pub fn new() -> Self {
        Self { editor_state: None }
    }

    pub fn with_editor_state(mut self, state: EditorStateRef) -> Self {
        self.editor_state = Some(state);
        self
    }

    fn create_parser() -> tree_sitter::Parser {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to load R grammar");
        parser
    }

    /// Check if the syntax tree indicates an incomplete expression.
    /// Returns true if the expression appears incomplete.
    ///
    /// We distinguish between:
    /// - Incomplete expressions (MISSING nodes, or ERROR at end) → Incomplete
    /// - Syntax errors (ERROR not at end) → Complete (let R report the error)
    fn is_incomplete(&self, tree: &tree_sitter::Tree, source: &[u8]) -> bool {
        let root = tree.root_node();

        if !root.has_error() {
            return false;
        }

        // Walk the tree to check for MISSING nodes or trailing ERRORs
        let mut cursor = root.walk();
        self.check_incomplete(&mut cursor, source)
    }

    /// Recursively check for signs of incomplete input:
    /// - MISSING nodes (tree-sitter inserted expected tokens)
    /// - ERROR nodes that extend to the end of meaningful content
    fn check_incomplete(&self, cursor: &mut tree_sitter::TreeCursor, source: &[u8]) -> bool {
        let node = cursor.node();

        // Find the end of meaningful content (ignoring trailing whitespace)
        let content_end = source
            .iter()
            .rposition(|&b| !b.is_ascii_whitespace())
            .map(|i| i + 1)
            .unwrap_or(0);

        // MISSING nodes always indicate incomplete input
        // (tree-sitter inserted an expected token that wasn't there)
        if node.is_missing() {
            return true;
        }

        // ERROR nodes at the end of meaningful content suggest incomplete input
        // ERROR nodes NOT at the end are syntax errors (let R report them)
        if node.kind() == "ERROR" && node.end_byte() >= content_end {
            return true;
        }

        // Recursively check children
        if cursor.goto_first_child() {
            loop {
                if self.check_incomplete(cursor, source) {
                    return true;
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }

        false
    }

    /// Detect misparsed raw strings.
    ///
    /// Tree-sitter may parse incomplete raw strings like `r"(\n"` as:
    ///   (program (identifier) (string ...))
    /// where `r` is an identifier and `"(\n"` is a regular string.
    ///
    /// This function detects this pattern:
    /// 1. First child is identifier "r" or "R"
    /// 2. Second child is a string that starts with a raw string delimiter
    ///
    /// Raw string delimiters in R: `(`, `[`, `{`, or `-` followed by one of these
    ///
    /// TODO: This is a workaround for a tree-sitter-r parsing issue.
    /// When tree-sitter-r is fixed to properly recognize incomplete raw strings,
    /// this function can be removed. See: https://github.com/r-lib/tree-sitter-r
    fn is_misparsed_raw_string(&self, root: &tree_sitter::Node, source: &[u8]) -> bool {
        // Need at least 2 children
        if root.child_count() < 2 {
            return false;
        }

        let first = match root.child(0) {
            Some(n) => n,
            None => return false,
        };
        let second = match root.child(1) {
            Some(n) => n,
            None => return false,
        };

        // First child must be identifier "r" or "R"
        if first.kind() != "identifier" {
            return false;
        }
        let id_text = &source[first.start_byte()..first.end_byte()];
        if id_text != b"r" && id_text != b"R" {
            return false;
        }

        // Second child must be a string
        if second.kind() != "string" {
            return false;
        }

        // Get the string content (after the opening quote)
        let string_start = second.start_byte();
        if string_start + 1 >= source.len() {
            return false;
        }

        // Check if string starts with quote followed by raw string delimiter
        // The string node starts with ", so check the char after "
        let after_quote = source.get(string_start + 1).copied();
        let is_raw_delimiter = matches!(after_quote, Some(b'(') | Some(b'[') | Some(b'{') | Some(b'-'));

        if is_raw_delimiter {
            // This looks like a misparsed raw string
            // Check if the raw string is properly closed
            // A complete raw string would be parsed as a single (string) node, not (identifier)(string)
            // So if we got here, it's incomplete
            return true;
        }

        false
    }
}

impl Default for RValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl Validator for RValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        // Synchronize editor state with actual buffer content.
        // This helps detect when reedline has modified the buffer without
        // going through our parse_event (e.g., Enter in default mode).
        if let Some(ref state_ref) = self.editor_state {
            if let Ok(mut state) = state_ref.lock() {
                if line.is_empty() {
                    // Empty buffer means new prompt - reset state completely.
                    // This is the only safe time to clear uncertain.
                    state.reset();
                } else if state.buffer != line {
                    // Buffer differs - sync and mark uncertain since we don't
                    // know the cursor position.
                    state.buffer = line.to_string();
                    state.buffer_len = line.chars().count();
                    state.uncertain = true;
                }
                // Note: We don't clear uncertain for non-empty matching buffers,
                // because reedline may add a newline AFTER this validator returns
                // (when validation is Incomplete). Clearing uncertain would
                // leave us with stale state thinking it's accurate.
            }
        }

        let escaped = escape_for_debug(line);

        // Empty lines are considered complete
        if line.trim().is_empty() {
            debug_log(&format!("[Validator] {:?} -> Complete (empty)", escaped));
            return ValidationResult::Complete;
        }

        let source = line.as_bytes();

        // Parse with tree-sitter (doesn't call R, avoids re-entrancy)
        let mut parser = Self::create_parser();
        let tree = match parser.parse(source, None) {
            Some(tree) => tree,
            None => {
                debug_log(&format!("[Validator] {:?} -> Complete (parse failed)", escaped));
                return ValidationResult::Complete;
            }
        };

        let root = tree.root_node();
        debug_log(&format!(
            "[Validator] {:?} has_error={} tree={}",
            escaped,
            root.has_error(),
            root.to_sexp()
        ));

        // Check for incomplete raw strings that tree-sitter misparses
        if self.is_misparsed_raw_string(&root, source) {
            debug_log(&format!("[Validator] {:?} -> Incomplete (misparsed raw string)", escaped));
            return ValidationResult::Incomplete;
        }

        let is_incomplete = self.is_incomplete(&tree, source);
        let result = if is_incomplete {
            ValidationResult::Incomplete
        } else {
            ValidationResult::Complete
        };

        debug_log(&format!(
            "[Validator] {:?} -> {}",
            escaped,
            if is_incomplete { "Incomplete" } else { "Complete" }
        ));

        result
    }
}

// =============================================================================
// Debug utilities for validator troubleshooting
// =============================================================================

/// Write debug log to file.
///
/// Only active when `R_TERM_VALIDATOR_DEBUG=1` environment variable is set.
/// Logs are written to `arf-validator.log` in the system temp directory.
fn debug_log(msg: &str) {
    use std::io::Write;
    use std::sync::OnceLock;

    static DEBUG_ENABLED: OnceLock<bool> = OnceLock::new();
    static LOG_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();

    let enabled = DEBUG_ENABLED.get_or_init(|| {
        std::env::var("R_TERM_VALIDATOR_DEBUG")
            .map(|v| v == "1")
            .unwrap_or(false)
    });

    if !*enabled {
        return;
    }

    let path = LOG_PATH.get_or_init(|| std::env::temp_dir().join("arf-validator.log"));

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
    }
}

/// Escape a string for debug logging (convert control characters to visible form).
/// Used by debug_log when R_TERM_VALIDATOR_DEBUG=1.
#[allow(dead_code)]
fn escape_for_debug(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            c if c.is_ascii_graphic() || c == ' ' => c.to_string(),
            c => format!("\\x{:02x}", c as u32),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_complete(result: ValidationResult) -> bool {
        matches!(result, ValidationResult::Complete)
    }

    fn is_incomplete(result: ValidationResult) -> bool {
        matches!(result, ValidationResult::Incomplete)
    }

    #[test]
    fn test_validator_complete_expressions() {
        let validator = RValidator::new();

        // Simple complete expressions
        assert!(is_complete(validator.validate("1 + 1")));
        assert!(is_complete(validator.validate("x <- 1")));
        assert!(is_complete(validator.validate("print(x)")));
        assert!(is_complete(validator.validate("stop('Test error')")));

        // Function calls
        assert!(is_complete(validator.validate("mean(c(1, 2, 3))")));
        assert!(is_complete(validator.validate("list(a = 1, b = 2)")));

        // Blocks
        assert!(is_complete(validator.validate("{ x <- 1; x }")));
        assert!(is_complete(validator.validate("if (TRUE) 1 else 2")));
    }

    #[test]
    fn test_validator_incomplete_expressions() {
        let validator = RValidator::new();

        // Unclosed parentheses
        assert!(is_incomplete(validator.validate("foo(")));
        assert!(is_incomplete(validator.validate("mean(c(1, 2")));

        // Unclosed braces
        assert!(is_incomplete(validator.validate("{")));
        assert!(is_incomplete(validator.validate("function() {")));

        // Unclosed brackets
        assert!(is_incomplete(validator.validate("x[")));

        // Unclosed strings
        assert!(is_incomplete(validator.validate("\"hello")));
        assert!(is_incomplete(validator.validate("'world")));

        // Trailing operators
        assert!(is_incomplete(validator.validate("1 +")));
        assert!(is_incomplete(validator.validate("x <-")));
    }

    #[test]
    fn test_validator_meta_commands_are_complete() {
        let validator = RValidator::new();

        // Meta commands should be treated as complete (even if they're not valid R)
        // so that they can be processed by the meta command handler
        assert!(is_complete(validator.validate(":h")));
        assert!(is_complete(validator.validate(":help")));
        assert!(is_complete(validator.validate(":quit")));
    }

    #[test]
    fn test_validator_empty_and_whitespace() {
        let validator = RValidator::new();

        assert!(is_complete(validator.validate("")));
        assert!(is_complete(validator.validate("   ")));
        assert!(is_complete(validator.validate("\t")));
    }

    #[test]
    fn test_validator_raw_strings() {
        let validator = RValidator::new();

        // Complete raw strings
        assert!(is_complete(validator.validate(r#"r"(hello)""#)));
        assert!(is_complete(validator.validate(r#"r"-(hello)-""#)));
        assert!(is_complete(validator.validate(r#"r"(')""#)));
        assert!(is_complete(validator.validate(r#"r"()""#)));

        // Incomplete raw strings
        assert!(is_incomplete(validator.validate(r#"r"(hello"#)));
        // Raw string with content that looks like it could be closed but isn't
        assert!(is_incomplete(validator.validate(concat!(r#"r"(')"#, "\n"))));
        assert!(is_incomplete(validator.validate(concat!(r#"r"(')"#, "\n\n"))));

        // Raw strings with quotes inside (reported issue)
        // r"( on first line, then quotes on second line - all should be incomplete
        assert!(is_incomplete(validator.validate(concat!(r#"r"("#, "\n", r#"""#))));   // one quote
        assert!(is_incomplete(validator.validate(concat!(r#"r"("#, "\n", r#""""#))));  // two quotes
        assert!(is_incomplete(validator.validate(concat!(r#"r"("#, "\nhello", r#"""#)))); // text + quote

        // Complete: r"( followed by )" closes it
        assert!(is_complete(validator.validate(concat!(r#"r"("#, "\n", r#")""#))));
        assert!(is_complete(validator.validate(concat!(r#"r"(""#, "\n", r#")""#))));
    }


    #[test]
    fn test_validator_multiline() {
        let validator = RValidator::new();

        // Single open paren
        assert!(is_incomplete(validator.validate("(")));

        // Open paren with newlines
        assert!(is_incomplete(validator.validate("(\n")));
        assert!(is_incomplete(validator.validate("(\n\n")));
        assert!(is_incomplete(validator.validate("(\n\n\n")));

        // Open paren with content on continuation line
        assert!(is_incomplete(validator.validate("(\n1")));
        assert!(is_complete(validator.validate("(\n1\n)")));

        // Function definition
        assert!(is_incomplete(validator.validate("function() {")));
        assert!(is_incomplete(validator.validate("function() {\n")));
        assert!(is_complete(validator.validate("function() {\n1\n}")));
    }
}
