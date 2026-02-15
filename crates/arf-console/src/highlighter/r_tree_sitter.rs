//! Tree-sitter based R syntax highlighting.
//!
//! This module implements R syntax highlighting using tree-sitter-r,
//! providing more accurate parsing than the regex-based approach.
//!
//! This highlighter also synchronizes the editor shadow state with the
//! actual buffer content on every redraw, enabling accurate bracket pair
//! detection even after history navigation.

use crate::config::RColorConfig;
use crate::editor::mode::EditorStateRef;
use crate::r_parser::{is_atomic_node, parse_r};
use nu_ansi_term::Style;
use once_cell::sync::Lazy;
use reedline::{Highlighter, StyledText};
use std::collections::HashSet;
use tree_sitter::Node;

use super::r_regex::TokenType;

/// Reserved keywords in R.
static KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "if", "else", "for", "while", "repeat", "in", "next", "break", "return", "function",
    ]
    .into_iter()
    .collect()
});

/// Built-in constants in R.
static CONSTANTS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "TRUE",
        "FALSE",
        "NULL",
        "Inf",
        "NaN",
        "NA",
        "NA_integer_",
        "NA_real_",
        "NA_complex_",
        "NA_character_",
    ]
    .into_iter()
    .collect()
});

/// A token with its position and type.
#[derive(Debug, Clone)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub token_type: TokenType,
}

// ---------------------------------------------------------------------------
// Free functions for tokenization (shared between RTreeSitterHighlighter
// and the public tokenize_r API).
// ---------------------------------------------------------------------------

/// Map a tree-sitter node kind to our TokenType.
fn node_to_token_type(node: &Node, source: &[u8]) -> TokenType {
    match node.kind() {
        // Literals
        "integer" | "float" | "complex" => TokenType::Number,
        "string" | "string_content" => TokenType::String,
        "escape_sequence" => TokenType::String,

        // Comments
        "comment" => TokenType::Comment,

        // Constants - these are named nodes in tree-sitter-r
        "true" | "false" => TokenType::Constant,
        "null" | "inf" | "nan" | "na" => TokenType::Constant,
        "dots" | "dot_dot_i" => TokenType::Constant,

        // Keywords
        "function" | "if" | "else" | "for" | "while" | "repeat" | "in" | "next" | "break"
        | "return" => TokenType::Keyword,

        // Operators
        "?" | ":=" | "=" | "<-" | "<<-" | "->" | "->>" | "~" | "|>" | "||" | "|" | "&&" | "&"
        | "<" | "<=" | ">" | ">=" | "==" | "!=" | "+" | "-" | "*" | "/" | "::" | ":::" | "**"
        | "^" | "$" | "@" | ":" | "!" | "\\" | "special" => TokenType::Operator,

        // Punctuation
        "(" | ")" | "{" | "}" | "[" | "]" | "[[" | "]]" => TokenType::Punctuation,
        "comma" | ";" => TokenType::Punctuation,

        // Identifiers - check if it's a keyword or constant
        "identifier" => {
            let text = node.utf8_text(source).unwrap_or("");
            if KEYWORDS.contains(text) {
                TokenType::Keyword
            } else if CONSTANTS.contains(text) {
                TokenType::Constant
            } else {
                TokenType::Identifier
            }
        }

        // Default
        _ => TokenType::Other,
    }
}

/// Recursively visit nodes and collect leaf tokens.
fn visit_node(cursor: &mut tree_sitter::TreeCursor, source: &[u8], tokens: &mut Vec<Token>) {
    let node = cursor.node();

    // For atomic nodes, treat as a whole (don't recurse into children)
    let kind = node.kind();
    if is_atomic_node(kind) {
        tokens.push(Token {
            start: node.start_byte(),
            end: node.end_byte(),
            token_type: node_to_token_type(&node, source),
        });
        return;
    }

    // If this is a leaf node (no children), add it as a token
    if node.child_count() == 0 {
        let token_type = node_to_token_type(&node, source);
        // Only add non-trivial tokens
        if token_type != TokenType::Other || node.start_byte() < node.end_byte() {
            tokens.push(Token {
                start: node.start_byte(),
                end: node.end_byte(),
                token_type,
            });
        }
    } else {
        // Recurse into children
        if cursor.goto_first_child() {
            loop {
                visit_node(cursor, source, tokens);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
    }
}

/// Fill gaps between tokens with whitespace.
fn fill_gaps(tokens: &[Token], total_len: usize) -> Vec<Token> {
    let mut result = Vec::new();
    let mut pos = 0;

    for token in tokens.iter() {
        // Add whitespace for any gap
        if token.start > pos {
            result.push(Token {
                start: pos,
                end: token.start,
                token_type: TokenType::Whitespace,
            });
        }
        result.push(token.clone());
        pos = token.end;
    }

    // Add trailing whitespace if needed
    if pos < total_len {
        result.push(Token {
            start: pos,
            end: total_len,
            token_type: TokenType::Whitespace,
        });
    }

    result
}

/// Tokenize R source code into a flat list of tokens.
///
/// Each token has a byte range and a [`TokenType`]. The tokens cover the
/// entire input (gaps are filled with `Whitespace` tokens).
///
/// Uses the shared thread-local tree-sitter parser from [`crate::r_parser`].
pub fn tokenize_r(source: &str) -> Vec<Token> {
    let tree = match parse_r(source) {
        Some(t) => t,
        None => {
            return vec![Token {
                start: 0,
                end: source.len(),
                token_type: TokenType::Other,
            }];
        }
    };

    let source_bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = tree.walk();

    visit_node(&mut cursor, source_bytes, &mut tokens);
    tokens.sort_by_key(|t| t.start);
    fill_gaps(&tokens, source.len())
}

/// Tree-sitter based R syntax highlighter.
///
/// This highlighter can optionally sync editor shadow state on every redraw,
/// keeping the state accurate even after history navigation.
pub struct RTreeSitterHighlighter {
    config: RColorConfig,
    /// Optional editor state reference for syncing on redraw.
    editor_state: Option<EditorStateRef>,
}

impl RTreeSitterHighlighter {
    pub fn new(config: RColorConfig) -> Self {
        RTreeSitterHighlighter {
            config,
            editor_state: None,
        }
    }

    /// Set the editor state reference for shadow state synchronization.
    ///
    /// When set, the highlighter will sync the editor state with the actual
    /// buffer content and cursor position on every redraw. This ensures
    /// accurate state tracking even after history navigation.
    pub fn with_editor_state(mut self, state: EditorStateRef) -> Self {
        self.editor_state = Some(state);
        self
    }

    /// Synchronize the shadow state with the actual buffer content.
    fn sync_editor_state(&self, line: &str, cursor: usize) {
        if let Some(state_ref) = &self.editor_state
            && let Ok(mut state) = state_ref.lock()
        {
            // Update shadow state to match actual buffer
            state.buffer = line.to_string();
            state.buffer_len = line.chars().count();
            // Convert byte position to char position
            state.cursor_pos = line[..cursor.min(line.len())].chars().count();
            state.uncertain = false;
        }
    }
}

impl Default for RTreeSitterHighlighter {
    fn default() -> Self {
        Self::new(RColorConfig::default())
    }
}

impl Highlighter for RTreeSitterHighlighter {
    fn highlight(&self, line: &str, cursor: usize) -> StyledText {
        // Sync editor state with actual buffer on every redraw.
        // This ensures accurate state tracking after history navigation.
        self.sync_editor_state(line, cursor);

        let mut styled = StyledText::new();

        if let Some(tree) = parse_r(line) {
            let source = line.as_bytes();
            let mut tokens = Vec::new();
            let mut cursor = tree.walk();

            // Collect tokens using free functions
            visit_node(&mut cursor, source, &mut tokens);

            // Sort and fill gaps
            tokens.sort_by_key(|t| t.start);
            let tokens = fill_gaps(&tokens, source.len());

            for token in tokens {
                if token.start < line.len() && token.end <= line.len() {
                    let text = &line[token.start..token.end];
                    let style = token.token_type.style(&self.config);
                    styled.push((style, text.to_string()));
                }
            }
        } else {
            // Fallback: no highlighting
            styled.push((Style::new(), line.to_string()));
        }

        // Handle empty input
        if styled.buffer.is_empty() {
            styled.push((Style::new(), String::new()));
        }

        styled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RColorConfig;
    use nu_ansi_term::Color;

    fn get_token_types(input: &str) -> Vec<(String, TokenType)> {
        tokenize_r(input)
            .into_iter()
            .filter(|t| t.token_type != TokenType::Whitespace)
            .map(|t| {
                let text = input[t.start..t.end].to_string();
                (text, t.token_type)
            })
            .collect()
    }

    #[test]
    fn test_comment() {
        let tokens = get_token_types("# this is a comment");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].1, TokenType::Comment);
    }

    #[test]
    fn test_string() {
        let tokens = get_token_types(r#""hello world""#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].1, TokenType::String);
    }

    #[test]
    fn test_raw_string() {
        let tokens = get_token_types(r#"r"(hello "world")""#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].1, TokenType::String);
    }

    #[test]
    fn test_numbers() {
        let cases = vec!["42", "3.14", "1e-5", "0xFF", "1L", "2i"];
        for case in cases {
            let tokens = get_token_types(case);
            assert_eq!(tokens[0].1, TokenType::Number, "Failed for: {}", case);
        }
    }

    #[test]
    fn test_keywords() {
        let tokens = get_token_types("if (TRUE) else FALSE");
        let keywords: Vec<_> = tokens
            .iter()
            .filter(|(_, t)| *t == TokenType::Keyword)
            .collect();
        assert_eq!(keywords.len(), 2); // if, else
    }

    #[test]
    fn test_constants() {
        let tokens = get_token_types("TRUE FALSE NULL NA Inf NaN");
        let constants: Vec<_> = tokens
            .iter()
            .filter(|(_, t)| *t == TokenType::Constant)
            .collect();
        assert_eq!(constants.len(), 6);
    }

    #[test]
    fn test_operators() {
        let tokens = get_token_types("x <- 1 + 2");
        let operators: Vec<_> = tokens
            .iter()
            .filter(|(_, t)| *t == TokenType::Operator)
            .collect();
        assert_eq!(operators.len(), 2); // <-, +
    }

    #[test]
    fn test_assignment() {
        let tokens = get_token_types("x <- 42");
        assert_eq!(tokens[0], ("x".to_string(), TokenType::Identifier));
        assert_eq!(tokens[1], ("<-".to_string(), TokenType::Operator));
        assert_eq!(tokens[2], ("42".to_string(), TokenType::Number));
    }

    #[test]
    fn test_highlight_preserves_text() {
        let highlighter = RTreeSitterHighlighter::default();
        let input = "x <- c(1, 2, 3)";
        let styled = highlighter.highlight(input, 0);
        assert_eq!(styled.raw_string(), input);
    }

    #[test]
    fn test_highlight_empty() {
        let highlighter = RTreeSitterHighlighter::default();
        let styled = highlighter.highlight("", 0);
        assert_eq!(styled.raw_string(), "");
    }

    // Custom color configuration tests

    #[test]
    fn test_custom_keyword_color() {
        let config = RColorConfig {
            keyword: Color::Red,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("if", 0);

        // Find the "if" segment and verify it has Red foreground
        let if_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "if")
            .expect("Should find 'if' segment");
        assert_eq!(
            if_segment.0.foreground,
            Some(Color::Red),
            "Keyword 'if' should be styled with Red"
        );
    }

    #[test]
    fn test_custom_string_color() {
        let config = RColorConfig {
            string: Color::Yellow,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight(r#""hello""#, 0);

        let string_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text.contains("hello"))
            .expect("Should find string segment");
        assert_eq!(
            string_segment.0.foreground,
            Some(Color::Yellow),
            "String should be styled with Yellow"
        );
    }

    #[test]
    fn test_custom_number_color() {
        let config = RColorConfig {
            number: Color::Blue,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("42", 0);

        let number_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "42")
            .expect("Should find number segment");
        assert_eq!(
            number_segment.0.foreground,
            Some(Color::Blue),
            "Number should be styled with Blue"
        );
    }

    #[test]
    fn test_custom_comment_color() {
        let config = RColorConfig {
            comment: Color::Cyan,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("# comment", 0);

        let comment_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text.contains("comment"))
            .expect("Should find comment segment");
        assert_eq!(
            comment_segment.0.foreground,
            Some(Color::Cyan),
            "Comment should be styled with Cyan"
        );
    }

    #[test]
    fn test_custom_constant_color() {
        let config = RColorConfig {
            constant: Color::Magenta,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("TRUE", 0);

        let constant_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "TRUE")
            .expect("Should find constant segment");
        assert_eq!(
            constant_segment.0.foreground,
            Some(Color::Magenta),
            "Constant TRUE should be styled with Magenta"
        );
    }

    #[test]
    fn test_custom_operator_color() {
        let config = RColorConfig {
            operator: Color::Green,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("x <- 1", 0);

        let operator_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "<-")
            .expect("Should find operator segment");
        assert_eq!(
            operator_segment.0.foreground,
            Some(Color::Green),
            "Operator <- should be styled with Green"
        );
    }

    #[test]
    fn test_custom_identifier_color() {
        let config = RColorConfig {
            identifier: Color::White,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("myvar", 0);

        let identifier_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "myvar")
            .expect("Should find identifier segment");
        assert_eq!(
            identifier_segment.0.foreground,
            Some(Color::White),
            "Identifier should be styled with White"
        );
    }

    #[test]
    fn test_default_color_no_styling() {
        // Color::Default should result in no foreground color (None)
        let config = RColorConfig {
            identifier: Color::Default,
            punctuation: Color::Default,
            ..Default::default()
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("x()", 0);

        // Identifier 'x' should have no foreground color
        let identifier_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "x")
            .expect("Should find identifier segment");
        assert_eq!(
            identifier_segment.0.foreground, None,
            "Color::Default should result in no foreground color"
        );

        // Punctuation should have no foreground color
        let paren_segment = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "(")
            .expect("Should find parenthesis segment");
        assert_eq!(
            paren_segment.0.foreground, None,
            "Punctuation with Color::Default should have no foreground color"
        );
    }

    #[test]
    fn test_multiple_custom_colors() {
        // Test that multiple custom colors work together
        let config = RColorConfig {
            keyword: Color::Red,
            constant: Color::Blue,
            operator: Color::Green,
            number: Color::Yellow,
            identifier: Color::White,
            comment: Color::DarkGray,
            string: Color::Cyan,
            punctuation: Color::Magenta,
        };
        let highlighter = RTreeSitterHighlighter::new(config);
        let styled = highlighter.highlight("if (x <- 1) TRUE", 0);

        // Check keyword
        let if_seg = styled.buffer.iter().find(|(_, t)| t == "if");
        assert_eq!(if_seg.map(|(s, _)| s.foreground), Some(Some(Color::Red)));

        // Check operator
        let op_seg = styled.buffer.iter().find(|(_, t)| t == "<-");
        assert_eq!(op_seg.map(|(s, _)| s.foreground), Some(Some(Color::Green)));

        // Check number
        let num_seg = styled.buffer.iter().find(|(_, t)| t == "1");
        assert_eq!(
            num_seg.map(|(s, _)| s.foreground),
            Some(Some(Color::Yellow))
        );

        // Check constant
        let const_seg = styled.buffer.iter().find(|(_, t)| t == "TRUE");
        assert_eq!(
            const_seg.map(|(s, _)| s.foreground),
            Some(Some(Color::Blue))
        );

        // Check identifier
        let id_seg = styled.buffer.iter().find(|(_, t)| t == "x");
        assert_eq!(id_seg.map(|(s, _)| s.foreground), Some(Some(Color::White)));

        // Check punctuation
        let paren_seg = styled.buffer.iter().find(|(_, t)| t == "(");
        assert_eq!(
            paren_seg.map(|(s, _)| s.foreground),
            Some(Some(Color::Magenta))
        );
    }
}
