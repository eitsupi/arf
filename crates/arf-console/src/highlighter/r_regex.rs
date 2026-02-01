//! Regex-based R syntax highlighting.
//!
//! This module implements R syntax highlighting using regex patterns,
//! ported from radian's Pygments-based lexer.
//!
//! Note: This module is kept as a fallback. The primary highlighter is now
//! tree-sitter based (r_tree_sitter.rs).

#![allow(dead_code)]

use crate::config::RColorConfig;
use nu_ansi_term::{Color, Style};
use once_cell::sync::Lazy;
use reedline::{Highlighter, StyledText};
use regex::Regex;
use std::collections::HashSet;

/// Token types for R syntax highlighting.
/// Based on tree-sitter-r's highlights.scm definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    /// Comments starting with #
    Comment,
    /// String literals (single or double quoted, including raw strings)
    String,
    /// Numeric literals (integers, floats, hex, complex)
    Number,
    /// Reserved keywords (if, else, for, while, function, etc.)
    Keyword,
    /// Built-in constants (TRUE, FALSE, NULL, NA, Inf, NaN)
    Constant,
    /// Operators (+, -, <-, |>, %%, etc.)
    Operator,
    /// Punctuation (brackets, commas, semicolons)
    Punctuation,
    /// Identifiers (variable names, function names)
    Identifier,
    /// Whitespace
    Whitespace,
    /// Unrecognized text
    Other,
}

impl TokenType {
    /// Get the style for this token type from configuration.
    pub fn style(self, config: &RColorConfig) -> Style {
        match self {
            TokenType::Comment => color_to_style(config.comment),
            TokenType::String => color_to_style(config.string),
            TokenType::Number => color_to_style(config.number),
            TokenType::Keyword => color_to_style(config.keyword),
            TokenType::Constant => color_to_style(config.constant),
            TokenType::Operator => color_to_style(config.operator),
            TokenType::Punctuation => color_to_style(config.punctuation),
            TokenType::Identifier => color_to_style(config.identifier),
            TokenType::Whitespace => Style::new(),
            TokenType::Other => Style::new(),
        }
    }
}

/// Convert a Color to a Style with that color as foreground.
/// Color::Default results in no foreground color (plain text).
fn color_to_style(color: Color) -> Style {
    match color {
        Color::Default => Style::new(),
        c => Style::new().fg(c),
    }
}

/// A token with its position and type.
#[derive(Debug, Clone)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub token_type: TokenType,
}

/// Reserved keywords in R (from ?Reserved plus return which is special).
/// Based on tree-sitter-r grammar.js definitions.
static KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "if", "else", "for", "while", "repeat", "in", "next", "break", "return", "function",
    ]
    .into_iter()
    .collect()
});

/// Built-in constants in R (from tree-sitter-r highlights.scm).
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

/// Regex patterns for R syntax.
/// Order matters: patterns are checked in priority order.
static PATTERNS: Lazy<Vec<(Regex, TokenType)>> = Lazy::new(|| {
    vec![
        // Comments (highest priority - must come first)
        (Regex::new(r"^#.*").unwrap(), TokenType::Comment),
        // Whitespace
        (Regex::new(r"^\s+").unwrap(), TokenType::Whitespace),
        // Raw strings (R 4.0+) - must come before regular strings
        // Double-quoted raw strings with various delimiters
        (
            Regex::new(r#"^[rR]"-*\((?s:.)*?\)-*""#).unwrap(),
            TokenType::String,
        ),
        (
            Regex::new(r#"^[rR]"-*\[(?s:.)*?\]-*""#).unwrap(),
            TokenType::String,
        ),
        (
            Regex::new(r#"^[rR]"-*\{(?s:.)*?\}-*""#).unwrap(),
            TokenType::String,
        ),
        // Single-quoted raw strings with various delimiters
        (
            Regex::new(r"^[rR]'-*\((?s:.)*?\)-*'").unwrap(),
            TokenType::String,
        ),
        (
            Regex::new(r"^[rR]'-*\[(?s:.)*?\]-*'").unwrap(),
            TokenType::String,
        ),
        (
            Regex::new(r"^[rR]'-*\{(?s:.)*?\}-*'").unwrap(),
            TokenType::String,
        ),
        // Regular strings (with escape handling)
        (
            Regex::new(r#"^"([^"\\]|\\.)*""#).unwrap(),
            TokenType::String,
        ),
        (Regex::new(r"^'([^'\\]|\\.)*'").unwrap(), TokenType::String),
        // Unclosed strings (highlight to end of line)
        (
            Regex::new(r#"^"([^"\\]|\\.)*$"#).unwrap(),
            TokenType::String,
        ),
        (Regex::new(r"^'([^'\\]|\\.)*$").unwrap(), TokenType::String),
        // Special R symbols: ... and ..1, ..2, etc.
        (Regex::new(r"^\.\.\.").unwrap(), TokenType::Constant),
        (Regex::new(r"^\.\.[0-9]+").unwrap(), TokenType::Constant),
        // Hex numbers
        (
            Regex::new(r"^0[xX][a-fA-F0-9]+([pP][0-9]+)?[Li]?").unwrap(),
            TokenType::Number,
        ),
        // Decimal numbers (including scientific notation and complex)
        // Note: Leading +/- are NOT part of the number literal (they are unary operators)
        // This matches tree-sitter-r behavior
        (
            Regex::new(r"^([0-9]+(\.[0-9]+)?|\.[0-9]+)([eE][+-]?[0-9]+)?[Li]?").unwrap(),
            TokenType::Number,
        ),
        // Operators (order matters - longer patterns first)
        // Assignment, comparison, and namespace operators
        // Note: ::: must come before ::, ** must come before * in the alternation
        (
            Regex::new(r"^(<<-|<-|->>|->|:=|==|!=|<=|>=|&&|\|\||:::|::|[*][*])").unwrap(),
            TokenType::Operator,
        ),
        // Pipe operators
        (Regex::new(r"^(\|>|%>%)").unwrap(), TokenType::Operator),
        // Special operators (%...%)
        (Regex::new(r"^%[^%]*%").unwrap(), TokenType::Operator),
        // Lambda backslash (R 4.1+) - must come before single-char operators
        (Regex::new(r"^\\").unwrap(), TokenType::Operator),
        // Single-character operators
        (
            Regex::new(r"^[<>!&|?*+\^/=~$@:-]").unwrap(),
            TokenType::Operator,
        ),
        // Punctuation
        (
            Regex::new(r"^(\[\[|\]\]|\[|\]|\(|\)|;|,|\{|\})").unwrap(),
            TokenType::Punctuation,
        ),
        // Backtick-quoted identifiers
        (
            Regex::new(r"^`[^`\\]*(?:\\.[^`\\]*)*`").unwrap(),
            TokenType::Identifier,
        ),
        // Regular identifiers starting with letter
        (
            Regex::new(r"^[a-zA-Z][\w.]*").unwrap(),
            TokenType::Identifier,
        ),
        // Identifiers starting with dot followed by letter or underscore (not digit)
        // e.g., .hidden, .foo.bar
        (
            Regex::new(r"^\.[a-zA-Z_][\w.]*").unwrap(),
            TokenType::Identifier,
        ),
        // Single dot followed by nothing or non-word char is not an identifier
        // But a lone `.` is valid in R (used in formulas like y ~ .)
        (
            Regex::new(r"^\.(?:[^0-9\w]|$)").unwrap(),
            TokenType::Identifier,
        ),
    ]
});

/// Classify an identifier as keyword, constant, or regular identifier.
/// Based on tree-sitter-r definitions.
fn classify_identifier(text: &str) -> TokenType {
    if KEYWORDS.contains(text) {
        TokenType::Keyword
    } else if CONSTANTS.contains(text) {
        TokenType::Constant
    } else {
        TokenType::Identifier
    }
}

/// Tokenize R source code.
pub fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos = 0;

    while pos < input.len() {
        let remaining = &input[pos..];
        let mut matched = false;

        for (pattern, token_type) in PATTERNS.iter() {
            if let Some(m) = pattern.find(remaining)
                && m.start() == 0
                && !m.is_empty()
            {
                let text = &remaining[..m.len()];

                // Reclassify identifiers as keywords/constants if applicable
                let final_type = if *token_type == TokenType::Identifier {
                    classify_identifier(text)
                } else {
                    *token_type
                };

                tokens.push(Token {
                    start: pos,
                    end: pos + m.len(),
                    token_type: final_type,
                });
                pos += m.len();
                matched = true;
                break;
            }
        }

        // If no pattern matched, consume one character as Other
        if !matched {
            // Find the next character boundary
            let next_pos = input[pos..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| pos + i)
                .unwrap_or(input.len());
            tokens.push(Token {
                start: pos,
                end: next_pos,
                token_type: TokenType::Other,
            });
            pos = next_pos;
        }
    }

    tokens
}

/// R syntax highlighter using regex-based tokenization.
pub struct RHighlighter {
    config: RColorConfig,
}

impl RHighlighter {
    pub fn new(config: RColorConfig) -> Self {
        RHighlighter { config }
    }
}

impl Default for RHighlighter {
    fn default() -> Self {
        Self::new(RColorConfig::default())
    }
}

impl Highlighter for RHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        let tokens = tokenize(line);

        for token in tokens {
            let text = &line[token.start..token.end];
            let style = token.token_type.style(&self.config);
            styled.push((style, text.to_string()));
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

    #[test]
    fn test_tokenize_comment() {
        let tokens = tokenize("# this is a comment");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Comment);
    }

    #[test]
    fn test_tokenize_string_double() {
        let tokens = tokenize(r#""hello world""#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::String);
    }

    #[test]
    fn test_tokenize_string_single() {
        let tokens = tokenize("'hello world'");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::String);
    }

    #[test]
    fn test_tokenize_string_with_escape() {
        let tokens = tokenize(r#""hello \"world\"""#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::String);
    }

    #[test]
    fn test_tokenize_raw_string() {
        let tokens = tokenize(r#"r"(hello "world")""#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::String);
    }

    #[test]
    fn test_tokenize_keywords() {
        let input = "if else for while function";
        let tokens = tokenize(input);
        let keywords: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Keyword)
            .collect();
        assert_eq!(keywords.len(), 5);
    }

    #[test]
    fn test_tokenize_constants() {
        // Only R's built-in constants per tree-sitter-r (not pi, letters, LETTERS)
        let input = "TRUE FALSE NULL NA Inf NaN";
        let tokens = tokenize(input);
        let constants: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Constant)
            .collect();
        assert_eq!(constants.len(), 6);
    }

    #[test]
    fn test_tokenize_numbers() {
        let cases = vec!["42", "3.14", "1e-5", "0xFF", "1L", "2i", ".5", "1.5e+10"];
        for case in cases {
            let tokens = tokenize(case);
            assert_eq!(
                tokens[0].token_type,
                TokenType::Number,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_tokenize_operators() {
        let input = "<- -> |> %>% :: ::: == != <= >= && ||";
        let tokens = tokenize(input);
        let operators: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Operator)
            .map(|t| &input[t.start..t.end])
            .collect();
        assert_eq!(
            operators,
            vec![
                "<-", "->", "|>", "%>%", "::", ":::", "==", "!=", "<=", ">=", "&&", "||"
            ]
        );
    }

    #[test]
    fn test_tokenize_walrus_operator() {
        // := is used by data.table
        let tokens = tokenize("dt[, x := 1]");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Operator)
            .map(|t| &"dt[, x := 1]"[t.start..t.end])
            .collect();
        assert!(ops.contains(&":="));
    }

    #[test]
    fn test_tokenize_double_star() {
        // ** is an alternative to ^ for exponentiation
        let tokens = tokenize("2 ** 3");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Operator)
            .map(|t| &"2 ** 3"[t.start..t.end])
            .collect();
        assert_eq!(ops, vec!["**"]);
    }

    #[test]
    fn test_tokenize_lambda_backslash() {
        // \ is used for lambda functions in R 4.1+
        let tokens = tokenize(r"\(x) x + 1");
        assert_eq!(tokens[0].token_type, TokenType::Operator);
    }

    #[test]
    fn test_tokenize_special_operator() {
        let tokens = tokenize("%in%");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Operator);
    }

    #[test]
    fn test_tokenize_punctuation() {
        let input = "()[]{},;";
        let tokens = tokenize(input);
        let punct: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Punctuation)
            .collect();
        assert_eq!(punct.len(), 8);
    }

    #[test]
    fn test_tokenize_double_bracket() {
        let tokens = tokenize("x[[1]]");
        // Should tokenize as: identifier, [[, number, ]]
        assert_eq!(tokens.len(), 4);
    }

    #[test]
    fn test_tokenize_identifier() {
        let cases = vec!["foo", "bar_baz", ".hidden", "x.y.z"];
        for case in cases {
            let tokens = tokenize(case);
            assert_eq!(tokens.len(), 1, "Failed for: {}", case);
            assert_eq!(
                tokens[0].token_type,
                TokenType::Identifier,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_tokenize_data_frame() {
        // data.frame is a regular identifier (not a reserved word)
        let tokens = tokenize("data.frame");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Identifier);
    }

    #[test]
    fn test_tokenize_backtick_identifier() {
        let tokens = tokenize("`weird name`");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Identifier);
    }

    #[test]
    fn test_tokenize_function_call() {
        let input = "print(x)";
        let tokens = tokenize(input);
        // print, (, x, )
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].token_type, TokenType::Identifier);
        assert_eq!(tokens[1].token_type, TokenType::Punctuation);
        assert_eq!(tokens[2].token_type, TokenType::Identifier);
        assert_eq!(tokens[3].token_type, TokenType::Punctuation);
    }

    #[test]
    fn test_tokenize_assignment() {
        let input = "x <- 42";
        let tokens = tokenize(input);
        // x, space, <-, space, 42
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].token_type, TokenType::Identifier);
        assert_eq!(tokens[2].token_type, TokenType::Operator);
        assert_eq!(tokens[4].token_type, TokenType::Number);
    }

    #[test]
    fn test_tokenize_library_call() {
        // library is a regular function call, not a reserved keyword
        let input = "library(dplyr)";
        let tokens = tokenize(input);
        assert_eq!(tokens[0].token_type, TokenType::Identifier);
    }

    #[test]
    fn test_tokenize_na_variants() {
        let cases = vec![
            ("NA", TokenType::Constant),
            ("NA_integer_", TokenType::Constant),
            ("NA_real_", TokenType::Constant),
            ("NA_complex_", TokenType::Constant),
            ("NA_character_", TokenType::Constant),
        ];
        for (input, expected) in cases {
            let tokens = tokenize(input);
            assert_eq!(tokens[0].token_type, expected, "Failed for: {}", input);
        }
    }

    #[test]
    fn test_tokenize_dot_dot() {
        // ... and ..1, ..2, etc. are special R symbols
        let tokens = tokenize("...");
        assert_eq!(tokens[0].token_type, TokenType::Constant);

        let tokens = tokenize("..1");
        assert_eq!(tokens[0].token_type, TokenType::Constant);
    }

    #[test]
    fn test_highlight_basic() {
        let highlighter = RHighlighter::default();
        let styled = highlighter.highlight("x <- 42", 0);
        assert_eq!(styled.raw_string(), "x <- 42");
    }

    #[test]
    fn test_highlight_preserves_text() {
        let highlighter = RHighlighter::default();
        let input = "# comment\nx <- c(1, 2, 3)\nprint(x)";
        let styled = highlighter.highlight(input, 0);
        assert_eq!(styled.raw_string(), input);
    }

    #[test]
    fn test_highlight_empty_input() {
        let highlighter = RHighlighter::default();
        let styled = highlighter.highlight("", 0);
        assert_eq!(styled.raw_string(), "");
    }

    #[test]
    fn test_keyword_not_in_identifier() {
        // "iffy" should not match "if"
        let tokens = tokenize("iffy");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Identifier);
    }

    #[test]
    fn test_keyword_with_dot_suffix() {
        // "if.x" is an identifier, not keyword "if" followed by ".x"
        let tokens = tokenize("if.x");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Identifier);
    }

    #[test]
    fn test_t_and_f_are_identifiers() {
        // T and F are regular identifiers (not highlighted as constants per tree-sitter-r)
        let tokens = tokenize("T F");
        let identifiers: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Identifier)
            .collect();
        assert_eq!(identifiers.len(), 2);
    }

    #[test]
    fn test_tokenize_operator_no_spaces() {
        // Bug: `1+1` should tokenize as: 1, +, 1 (not: 1, +1)
        let input = "1+1";
        let tokens = tokenize(input);
        let token_texts: Vec<_> = tokens.iter().map(|t| &input[t.start..t.end]).collect();
        assert_eq!(
            token_texts,
            vec!["1", "+", "1"],
            "Operator should be separate token"
        );

        // Verify types
        assert_eq!(tokens[0].token_type, TokenType::Number);
        assert_eq!(tokens[1].token_type, TokenType::Operator);
        assert_eq!(tokens[2].token_type, TokenType::Number);
    }

    #[test]
    fn test_tokenize_operators_no_spaces_various() {
        // Test various operators without spaces
        let cases = vec![
            ("1-1", vec!["1", "-", "1"]),
            ("2*3", vec!["2", "*", "3"]),
            ("4/2", vec!["4", "/", "2"]),
            ("2^3", vec!["2", "^", "3"]),
            ("x<-1", vec!["x", "<-", "1"]),
            ("a==b", vec!["a", "==", "b"]),
        ];

        for (input, expected) in cases {
            let tokens = tokenize(input);
            let token_texts: Vec<_> = tokens.iter().map(|t| &input[t.start..t.end]).collect();
            assert_eq!(token_texts, expected, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_tokenize_unary_minus() {
        // Unary minus: `-1` should be: -, 1 (operator followed by number)
        // This matches tree-sitter-r behavior where - is a unary operator
        let input = "-1";
        let tokens = tokenize(input);
        let token_texts: Vec<_> = tokens.iter().map(|t| &input[t.start..t.end]).collect();
        assert_eq!(token_texts, vec!["-", "1"]);
        assert_eq!(tokens[0].token_type, TokenType::Operator);
        assert_eq!(tokens[1].token_type, TokenType::Number);
    }
}
