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
fn test_default_color_no_styling() {
    // Color::Default should result in no foreground color (None)
    let config = RColorConfig {
        identifier: Color::Default,
        punctuation: Color::Default,
        ..Default::default()
    };
    let highlighter = RTreeSitterHighlighter::new(config, false);
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
        matching_bracket: Color::LightYellow,
    };
    let highlighter = RTreeSitterHighlighter::new(config, false);
    let styled = highlighter.highlight(r#"if (x <- 1) TRUE; "text" # note"#, 0);

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

    // Check string
    let string_seg = styled.buffer.iter().find(|(_, t)| t.contains("text"));
    assert_eq!(
        string_seg.map(|(s, _)| s.foreground),
        Some(Some(Color::Cyan))
    );

    // Check comment
    let comment_seg = styled.buffer.iter().find(|(_, t)| t.contains("note"));
    assert_eq!(
        comment_seg.map(|(s, _)| s.foreground),
        Some(Some(Color::DarkGray))
    );
}

#[test]
fn test_bracket_highlight_applied() {
    let config = RColorConfig {
        punctuation: Color::Magenta,
        matching_bracket: Color::LightYellow,
        ..Default::default()
    };
    // Enable bracket highlighting
    let highlighter = RTreeSitterHighlighter::new(config, true);
    // Cursor on '(' at position 1 in "f(x)"
    let styled = highlighter.highlight("f(x)", 1);

    // Find the segment containing '(' and verify it has the highlight background
    let mut offset = 0;
    let mut found = false;
    for (style, text) in &styled.buffer {
        if offset <= 1 && offset + text.len() > 1 {
            found = true;
            assert_eq!(
                style.background,
                Some(Color::LightYellow),
                "Opening bracket should have highlight background"
            );
            assert_eq!(
                style.foreground,
                Some(Color::Magenta),
                "Opening bracket should preserve foreground color"
            );
            break;
        }
        offset += text.len();
    }
    assert!(found, "Should have found a segment containing the bracket");
}

#[test]
fn test_bracket_highlight_disabled() {
    let config = RColorConfig {
        matching_bracket: Color::LightYellow,
        ..Default::default()
    };
    // Disable bracket highlighting
    let highlighter = RTreeSitterHighlighter::new(config, false);
    let styled = highlighter.highlight("f(x)", 1);

    // No segment should have the highlight background
    for (style, _) in &styled.buffer {
        assert_ne!(
            style.background,
            Some(Color::LightYellow),
            "Bracket highlight should not be applied when disabled"
        );
    }
}
