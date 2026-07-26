use super::super::*;

// ===== Tests for skip-over rules =====

#[test]
fn test_skip_over_rules_created() {
    let rules = create_skip_over_rules();

    // Should have 6 rules: 3 for brackets (`)`, `]`, `}`) + 3 for quotes (`"`, `'`, `` ` ``)
    assert_eq!(rules.len(), 6);
}

/// Test that bracket skip-over rules match InsertChar events (not InsertString)
///
/// This is critical: bracket closing keys (`)`, `]`, `}`) generate InsertChar events,
/// while opening keys (`(`, `[`, `{`) generate InsertString + MoveLeft.
/// Skip-over rules must only match InsertChar to avoid interfering with opening brackets.
#[test]
fn test_skip_over_bracket_rules_match_insert_char() {
    let rules = create_skip_over_rules();

    // First 3 rules are for brackets: `)`, `]`, `}`
    for (i, close_char) in [')', ']', '}'].iter().enumerate() {
        let rule = &rules[i];

        // Should match InsertChar(close_char)
        let insert_char_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar(*close_char)]);
        assert!(
            (rule.match_event)(&insert_char_event),
            "Rule {} should match InsertChar('{}')",
            i,
            close_char
        );

        // Should NOT match InsertString(pair) + MoveLeft (opening bracket event)
        let pair = match close_char {
            ')' => "()",
            ']' => "[]",
            '}' => "{}",
            _ => unreachable!(),
        };
        let insert_string_event = ReedlineEvent::Edit(vec![
            EditCommand::InsertString(pair.to_string()),
            EditCommand::MoveLeft { select: false },
        ]);
        assert!(
            !(rule.match_event)(&insert_string_event),
            "Rule {} should NOT match InsertString for pair '{}'",
            i,
            pair
        );
    }
}

/// Test that quote skip-over rules match InsertString + MoveLeft events
///
/// Quotes use the same character for opening and closing, so they use
/// InsertString(pair) + MoveLeft for both.
#[test]
fn test_skip_over_quote_rules_match_insert_string() {
    let rules = create_skip_over_rules();

    // Last 3 rules are for quotes: `"`, `'`, `` ` ``
    let quote_pairs: [(char, &str); 3] = [('"', r#""""#), ('\'', "''"), ('`', "``")];
    for (i, (quote_char, pair)) in quote_pairs.iter().enumerate() {
        let rule = &rules[3 + i]; // Skip first 3 bracket rules

        // Should match InsertString(pair) + MoveLeft
        let insert_string_event = ReedlineEvent::Edit(vec![
            EditCommand::InsertString(pair.to_string()),
            EditCommand::MoveLeft { select: false },
        ]);
        assert!(
            (rule.match_event)(&insert_string_event),
            "Quote rule {} should match InsertString for pair '{}'",
            i,
            pair
        );

        // Should NOT match InsertChar(quote_char)
        let insert_char_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar(*quote_char)]);
        assert!(
            !(rule.match_event)(&insert_char_event),
            "Quote rule {} should NOT match InsertChar('{}')",
            i,
            quote_char
        );
    }
}

/// Regression test: opening bracket should insert pair even when cursor is before closing bracket
///
/// Bug scenario: `install.packages(c|)` -> type `(` -> should become `install.packages(c(|))`
/// Before fix: skip-over rule incorrectly matched the opening `(` key and moved cursor right
/// After fix: skip-over only matches InsertChar(')'), so `(` key works correctly
#[test]
fn test_opening_bracket_not_matched_by_skip_over_regression() {
    let skip_over_rules = create_skip_over_rules();
    let auto_match_rules = create_auto_match_rules();

    // The event generated when pressing `(` key (opening bracket)
    let open_paren_event = ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]);

    // Skip-over rule for `)` should NOT match the `(` key event
    let close_paren_skip_rule = &skip_over_rules[0]; // First rule is for `)`
    assert!(
        !(close_paren_skip_rule.match_event)(&open_paren_event),
        "Skip-over rule for ')' should NOT match opening '(' event"
    );

    // Auto-match rule for `(` should match the `(` key event
    let open_paren_auto_match_rule = &auto_match_rules[0]; // First rule is for `(`
    assert!(
        (open_paren_auto_match_rule.match_event)(&open_paren_event),
        "Auto-match rule for '(' should match opening '(' event"
    );
}

/// Test the full scenario: cursor before `)`, press `(`, should insert `()`
///
/// Simulates: `foo(c|)` -> press `(` -> `foo(c(|))`
#[test]
fn test_open_bracket_inside_parens_full_flow() {
    let mut state = EditorState::new();
    let skip_over_rules = create_skip_over_rules();
    let auto_match_rules = create_auto_match_rules();

    // Setup: `foo(c|)` - cursor before `)`
    state.buffer = "foo(c)".to_string();
    state.buffer_len = 6;
    state.cursor_pos = 5; // After 'c', before ')'

    assert_eq!(state.char_after_cursor(), Some(')'));
    assert!(!state.cursor_at_end());

    // The event for pressing `(` key
    let open_paren_event = ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]);

    // Skip-over rule for `)` should NOT match (it matches InsertChar, not InsertString)
    let close_paren_skip_rule = &skip_over_rules[0];
    assert!(!(close_paren_skip_rule.match_event)(&open_paren_event));

    // Auto-match rule for `(` should match
    let open_paren_auto_match_rule = &auto_match_rules[0];
    assert!((open_paren_auto_match_rule.match_event)(&open_paren_event));

    // Auto-match condition should pass (cursor is before closing char)
    assert!(
        open_paren_auto_match_rule.condition.check(&state),
        "Auto-match condition should pass when cursor is before ')'"
    );

    // Simulate the auto-match event execution
    state.update_from_event(&open_paren_event);

    // Result should be: `foo(c(|))`
    assert_eq!(state.buffer, "foo(c())");
    assert_eq!(state.cursor_pos, 6); // Between the new parens
}

/// Test that closing bracket skip-over works correctly
///
/// Simulates: `foo(|)` -> press `)` -> `foo()|`
#[test]
fn test_close_bracket_skip_over() {
    let mut state = EditorState::new();
    let skip_over_rules = create_skip_over_rules();

    // Setup: `foo(|)` - cursor before `)`
    state.buffer = "foo()".to_string();
    state.buffer_len = 5;
    state.cursor_pos = 4; // After '(', before ')'

    assert_eq!(state.char_after_cursor(), Some(')'));

    // The event for pressing `)` key (InsertChar)
    let close_paren_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar(')')]);

    // Skip-over rule for `)` should match
    let close_paren_skip_rule = &skip_over_rules[0];
    assert!((close_paren_skip_rule.match_event)(&close_paren_event));

    // Condition: CursorNotBeforeChar(')') - should be FALSE because cursor IS before ')'
    // When condition is false, fallback (MoveRight) is used
    assert!(
        !close_paren_skip_rule.condition.check(&state),
        "Condition should fail when cursor is before ')'"
    );

    // The fallback event is MoveRight, which skips over the existing ')'
    // Simulate that
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveRight {
        select: false,
    }]));

    assert_eq!(state.cursor_pos, 5); // Now at end
    assert_eq!(state.buffer, "foo()"); // Buffer unchanged
}
