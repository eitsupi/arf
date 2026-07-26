use super::super::*;

// ===== Tests for CursorAtEndOrBeforeClosing =====

#[test]
fn test_cursor_at_end_or_before_closing_at_end() {
    let condition = CursorAtEndOrBeforeClosing;

    // Empty buffer: cursor at end
    let mut state = EditorState::new();
    assert!(condition.check(&state));

    // Buffer with content, cursor at end
    state.buffer = "foo".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 3;
    assert!(condition.check(&state));
}

#[test]
fn test_cursor_at_end_or_before_closing_before_closing_chars() {
    let condition = CursorAtEndOrBeforeClosing;

    // Test each closing character
    let closing_chars = [')', ']', '}', '"', '\'', '`'];

    for closing_char in closing_chars {
        let mut state = EditorState::new();
        state.buffer = format!("x{}", closing_char);
        state.buffer_len = 2;
        state.cursor_pos = 1; // Between 'x' and closing char

        assert!(
            condition.check(&state),
            "Expected condition to pass when cursor is before '{}'",
            closing_char
        );
    }
}

#[test]
fn test_cursor_at_end_or_before_closing_before_regular_char() {
    let condition = CursorAtEndOrBeforeClosing;

    // Cursor before a regular character (not closing)
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 1; // Between 'a' and 'b'

    assert!(!condition.check(&state));
}

#[test]
fn test_cursor_at_end_or_before_closing_uncertain() {
    let condition = CursorAtEndOrBeforeClosing;

    // When uncertain and not at end, should return false
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;
    state.uncertain = true;

    // char_after_cursor returns None when uncertain
    assert!(!condition.check(&state));
}

// ===== Tests for nested bracket/quote scenarios =====

/// Test: `(│)` + `"` → `("│")`
#[test]
fn test_auto_match_quote_inside_parens() {
    let condition = CursorAtEndOrBeforeClosing;

    // State: `(│)` - cursor between parens
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1; // Before ')'

    // Condition should pass (cursor is before ')')
    assert!(condition.check(&state));

    // Simulate typing `"` with auto-match: InsertString("\"\"") + MoveLeft
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString(r#""""#.to_string()),
        EditCommand::MoveLeft { select: false },
    ]));

    // Result: `("│")`
    assert_eq!(state.buffer, r#"("")"#);
    assert_eq!(state.cursor_pos, 2); // Between the quotes
}

/// Test: `"│"` + `(` → `"(│)"`
#[test]
fn test_auto_match_parens_inside_quotes() {
    let condition = CursorAtEndOrBeforeClosing;

    // State: `"│"` - cursor between quotes
    let mut state = EditorState::new();
    state.buffer = r#""""#.to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1; // Before closing quote

    // Condition should pass (cursor is before '"')
    assert!(condition.check(&state));

    // Simulate typing `(` with auto-match: InsertString("()") + MoveLeft
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));

    // Result: `"(│)"`
    assert_eq!(state.buffer, r#""()""#);
    assert_eq!(state.cursor_pos, 2); // Between the parens
}

/// Test: `[│]` + `{` → `[{│}]`
#[test]
fn test_auto_match_braces_inside_brackets() {
    let condition = CursorAtEndOrBeforeClosing;

    // State: `[│]` - cursor between brackets
    let mut state = EditorState::new();
    state.buffer = "[]".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1; // Before ']'

    // Condition should pass (cursor is before ']')
    assert!(condition.check(&state));

    // Simulate typing `{` with auto-match
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("{}".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));

    // Result: `[{│}]`
    assert_eq!(state.buffer, "[{}]");
    assert_eq!(state.cursor_pos, 2);
}

/// Test: `r"│"` + `(` → `r"(│)"`
#[test]
fn test_auto_match_parens_inside_raw_string() {
    let condition = CursorAtEndOrBeforeClosing;

    // State: `r"│"` - cursor between quotes of raw string
    let mut state = EditorState::new();
    state.buffer = r#"r"""#.to_string();
    state.buffer_len = 3;
    state.cursor_pos = 2; // Before closing quote

    // Condition should pass (cursor is before '"')
    assert!(condition.check(&state));

    // Simulate typing `(` with auto-match
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));

    // Result: `r"(│)"`
    assert_eq!(state.buffer, r#"r"()""#);
    assert_eq!(state.cursor_pos, 3);
}

/// Test: `foo│` + `(` → `foo(│)` (cursor at end, keep working)
#[test]
fn test_auto_match_at_end_still_works() {
    let condition = CursorAtEndOrBeforeClosing;

    // State: `foo│` - cursor at end
    let mut state = EditorState::new();
    state.buffer = "foo".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 3; // At end

    // Condition should pass (cursor at end)
    assert!(condition.check(&state));

    // Simulate typing `(` with auto-match
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));

    // Result: `foo(│)`
    assert_eq!(state.buffer, "foo()");
    assert_eq!(state.cursor_pos, 4);
}

/// Test: `foo│bar` + `(` → `foo(│bar` (no auto-match in middle)
#[test]
fn test_auto_match_blocked_in_middle() {
    let condition = CursorAtEndOrBeforeClosing;

    // State: `foo│bar` - cursor in middle
    let mut state = EditorState::new();
    state.buffer = "foobar".to_string();
    state.buffer_len = 6;
    state.cursor_pos = 3; // Before 'b'

    // Condition should fail (cursor before 'b' which is not a closing char)
    assert!(!condition.check(&state));
}

/// Test: deeply nested brackets work correctly
#[test]
fn test_auto_match_deeply_nested() {
    let condition = CursorAtEndOrBeforeClosing;

    // Start with `(│)`, add multiple levels of nesting
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;

    // Add `[` inside parens: `([│])`
    assert!(condition.check(&state));
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("[]".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));
    assert_eq!(state.buffer, "([])");
    assert_eq!(state.cursor_pos, 2);

    // Add `{` inside brackets: `([{│}])`
    assert!(condition.check(&state));
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("{}".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));
    assert_eq!(state.buffer, "([{}])");
    assert_eq!(state.cursor_pos, 3);

    // Add `"` inside braces: `([{"│"}])`
    assert!(condition.check(&state));
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString(r#""""#.to_string()),
        EditCommand::MoveLeft { select: false },
    ]));
    assert_eq!(state.buffer, r#"([{""}])"#);
    assert_eq!(state.cursor_pos, 4);
}

#[test]
fn test_auto_match_rules_use_new_condition() {
    let rules = create_auto_match_rules();
    let paren_rule = &rules[0]; // '(' rule

    // Test that condition passes when cursor is before closing char
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1; // Before ')'

    // This should pass with the new CursorAtEndOrBeforeClosing condition
    assert!(paren_rule.condition.check(&state));

    // Test that condition fails when cursor is before regular char
    state.buffer = "ab".to_string();
    state.cursor_pos = 1; // Before 'b'

    assert!(!paren_rule.condition.check(&state));
}
