//! Tree-sitter based R syntax highlighting.
//!
//! This module implements R syntax highlighting using tree-sitter-r,
//! providing accurate parsing.
//!
use crate::config::RColorConfig;
use crate::r_parser::{is_atomic_node, parse_r, walk_atomic_tokens};
use nu_ansi_term::{Color, Style};
use once_cell::sync::Lazy;
use reedline::{Highlighter, StyledText};
use std::collections::HashSet;
use std::ops::ControlFlow;
use tree_sitter::Node;

use super::bracket_match::find_matching_bracket;
use super::token::TokenType;

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
        | "^" | "$" | "@" | ":" | "!" | r#"\"# | "special" => TokenType::Operator,

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

/// Collect the leaf tokens of a syntax tree, treating atomic nodes as a whole.
fn collect_tokens(tree: &tree_sitter::Tree, source: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    walk_atomic_tokens(tree.root_node(), |node| {
        let token_type = node_to_token_type(&node, source);
        // Atomic nodes are always kept; other leaves only when non-trivial.
        if is_atomic_node(node.kind())
            || token_type != TokenType::Other
            || node.start_byte() < node.end_byte()
        {
            tokens.push(Token {
                start: node.start_byte(),
                end: node.end_byte(),
                token_type,
            });
        }
        ControlFlow::Continue(())
    });
    tokens
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

    let mut tokens = collect_tokens(&tree, source.as_bytes());
    tokens.sort_by_key(|t| t.start);
    fill_gaps(&tokens, source.len())
}

/// Tree-sitter based R syntax highlighter.
///
pub struct RTreeSitterHighlighter {
    config: RColorConfig,
    highlight_matching_bracket: bool,
}

impl RTreeSitterHighlighter {
    pub fn new(config: RColorConfig, highlight_matching_bracket: bool) -> Self {
        RTreeSitterHighlighter {
            config,
            highlight_matching_bracket,
        }
    }
}

impl Default for RTreeSitterHighlighter {
    fn default() -> Self {
        Self::new(RColorConfig::default(), true)
    }
}

impl Highlighter for RTreeSitterHighlighter {
    fn highlight(&self, line: &str, cursor: usize) -> StyledText {
        let mut styled = StyledText::new();

        let tree = parse_r(line);

        if let Some(ref tree) = tree {
            let source = line.as_bytes();

            // Collect tokens using free functions
            let mut tokens = collect_tokens(tree, source);

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

        // Apply bracket highlighting after normal syntax highlighting.
        // This overlays a background color on the matching bracket pair
        // while preserving the existing foreground syntax color.
        if self.highlight_matching_bracket
            && self.config.matching_bracket != Color::Default
            && let Some(ref tree) = tree
            && let Some(bracket) = find_matching_bracket(line, cursor, tree)
        {
            let bg = self.config.matching_bracket;
            apply_bracket_highlight(&mut styled, bracket.cursor_bracket, bg);
            apply_bracket_highlight(&mut styled, bracket.matching_bracket, bg);
        }

        styled
    }
}

/// Apply bracket highlight to a single byte position in the styled text.
///
/// Reads the existing style at the position and merges it with the highlight
/// background color, preserving the existing foreground color.
fn apply_bracket_highlight(styled: &mut StyledText, pos: usize, highlight_bg: Color) {
    // Find the existing style at the byte position
    let existing_style = {
        let mut offset = 0;
        let mut found = None;
        for (style, text) in &styled.buffer {
            let end = offset + text.len();
            if pos >= offset && pos < end {
                found = Some(*style);
                break;
            }
            offset = end;
        }
        found.unwrap_or_default()
    };

    // Merge: keep existing foreground, set new background
    let merged = Style {
        background: Some(highlight_bg),
        ..existing_style
    };

    styled.style_range(pos, pos + 1, merged);
}

#[cfg(test)]
mod tests;
