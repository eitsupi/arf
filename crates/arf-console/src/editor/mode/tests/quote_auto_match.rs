use super::super::*;

// ===== Tests for cursor_in_quote and quote auto-match =====

#[test]
fn test_cursor_in_quote_empty() {
    let state = EditorState::new();
    assert!(!state.cursor_in_quote('"'));
    assert!(!state.cursor_in_quote('\''));
}

#[test]
fn test_cursor_in_quote_unclosed() {
    let mut state = EditorState::new();

    // `"foo|` - inside unclosed double quote
    state.buffer = r#""foo"#.to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;
    assert!(state.cursor_in_quote('"'));
    assert!(!state.cursor_in_quote('\'')); // Not inside single quote

    // `'foo|` - inside unclosed single quote
    state.buffer = "'foo".to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;
    assert!(state.cursor_in_quote('\''));
    assert!(!state.cursor_in_quote('"'));
}

#[test]
fn test_cursor_in_quote_closed() {
    let mut state = EditorState::new();

    // `"foo"|` - after closed string
    state.buffer = r#""foo""#.to_string();
    state.buffer_len = 5;
    state.cursor_pos = 5;
    assert!(!state.cursor_in_quote('"'));

    // `"foo" |` - after closed string with space
    state.buffer = r#""foo" "#.to_string();
    state.buffer_len = 6;
    state.cursor_pos = 6;
    assert!(!state.cursor_in_quote('"'));
}

#[test]
fn test_cursor_in_quote_escaped() {
    let mut state = EditorState::new();

    // `"foo\"bar|` - escaped quote, still inside string
    state.buffer = r#""foo\"bar"#.to_string();
    state.buffer_len = 9;
    state.cursor_pos = 9;
    assert!(state.cursor_in_quote('"'));

    // `"foo\"bar"|` - escaped quote, string closed
    state.buffer = r#""foo\"bar""#.to_string();
    state.buffer_len = 10;
    state.cursor_pos = 10;
    assert!(!state.cursor_in_quote('"'));
}

#[test]
fn test_cursor_in_quote_uncertain() {
    let mut state = EditorState::new();
    state.buffer = r#""foo"#.to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;
    state.uncertain = true;

    // When uncertain, should return false for safety
    assert!(!state.cursor_in_quote('"'));
}

#[test]
fn test_quote_auto_match_condition_not_in_string() {
    let condition = CursorAtEndOrBeforeClosingAndNotInQuote::new('"');

    // `foo|` - not in string, at end
    let mut state = EditorState::new();
    state.buffer = "foo".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 3;

    // Should allow auto-match
    assert!(condition.check(&state));
}

#[test]
fn test_quote_auto_match_condition_in_unclosed_string() {
    let condition = CursorAtEndOrBeforeClosingAndNotInQuote::new('"');

    // `"foo|` - inside unclosed string
    let mut state = EditorState::new();
    state.buffer = r#""foo"#.to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;

    // Should NOT allow auto-match (we want to just close the string)
    assert!(!condition.check(&state));
}

#[test]
fn test_quote_auto_match_condition_after_closed_string() {
    let condition = CursorAtEndOrBeforeClosingAndNotInQuote::new('"');

    // `"foo" |` - after closed string
    let mut state = EditorState::new();
    state.buffer = r#""foo" "#.to_string();
    state.buffer_len = 6;
    state.cursor_pos = 6;

    // Should allow auto-match (starting a new string)
    assert!(condition.check(&state));
}

/// Test: `"foo|` + `"` → `"foo"|` (close the string, don't insert pair)
#[test]
fn test_quote_auto_match_closes_string() {
    let rules = create_auto_match_rules();
    let quote_rule = &rules[3]; // '"' rule

    // State: `"foo|` - inside unclosed string
    let mut state = EditorState::new();
    state.buffer = r#""foo"#.to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;

    // Condition should fail (we're inside an unclosed string)
    assert!(!quote_rule.condition.check(&state));

    // The fallback should be InsertChar('"'), not InsertString("\"\"")
    match &quote_rule.fallback_event {
        ReedlineEvent::Edit(cmds) => {
            assert_eq!(cmds.len(), 1);
            assert!(matches!(&cmds[0], EditCommand::InsertChar('"')));
        }
        _ => panic!("Expected Edit event"),
    }
}

/// Test: `foo|` + `"` → `foo"|"` (not in string, auto-match works)
#[test]
fn test_quote_auto_match_works_outside_string() {
    let rules = create_auto_match_rules();
    let quote_rule = &rules[3]; // '"' rule

    // State: `foo|` - not inside any string
    let mut state = EditorState::new();
    state.buffer = "foo".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 3;

    // Condition should pass (not inside unclosed string, cursor at end)
    assert!(quote_rule.condition.check(&state));
}

/// Test: single quotes work the same way
#[test]
fn test_single_quote_auto_match_in_string() {
    let rules = create_auto_match_rules();
    let quote_rule = &rules[4]; // '\'' rule

    // State: `'foo|` - inside unclosed single-quoted string
    let mut state = EditorState::new();
    state.buffer = "'foo".to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;

    // Condition should fail
    assert!(!quote_rule.condition.check(&state));
}

/// Test: backticks work the same way
#[test]
fn test_backtick_auto_match_in_string() {
    let rules = create_auto_match_rules();
    let quote_rule = &rules[5]; // '`' rule

    // State: `` `foo| `` - inside unclosed backtick
    let mut state = EditorState::new();
    state.buffer = "`foo".to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;

    // Condition should fail
    assert!(!quote_rule.condition.check(&state));
}
