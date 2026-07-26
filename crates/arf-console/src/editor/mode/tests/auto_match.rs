use super::super::*;

#[test]
fn test_auto_match_rules_created() {
    let rules = create_auto_match_rules();

    // Should have 6 rules (for (, [, {, ", ', `)
    assert_eq!(rules.len(), 6);
}

#[test]
fn test_auto_match_rule_matches_paren() {
    let rules = create_auto_match_rules();
    let paren_rule = &rules[0]; // '(' rule

    // Should match the auto-match event for '('
    let match_event = ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]);
    assert!((paren_rule.match_event)(&match_event));

    // Should not match other events
    let other_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar('(')]);
    assert!(!(paren_rule.match_event)(&other_event));

    // Should not match different pair
    let bracket_event = ReedlineEvent::Edit(vec![
        EditCommand::InsertString("[]".to_string()),
        EditCommand::MoveLeft { select: false },
    ]);
    assert!(!(paren_rule.match_event)(&bracket_event));
}

#[test]
fn test_auto_match_condition_cursor_at_end() {
    let rules = create_auto_match_rules();
    let rule = &rules[0]; // '(' rule

    // Cursor at end: condition should pass
    let mut state = EditorState::new();
    state.buffer_len = 5;
    state.cursor_pos = 5;
    assert!(rule.condition.check(&state));

    // Cursor not at end: condition should fail
    state.cursor_pos = 2;
    assert!(!rule.condition.check(&state));
}

#[test]
fn test_auto_match_fallback_event() {
    let rules = create_auto_match_rules();

    // Check fallback events for each pair
    let expected_fallbacks = ['(', '[', '{', '"', '\'', '`'];
    for (rule, expected_char) in rules.iter().zip(expected_fallbacks.iter()) {
        match &rule.fallback_event {
            ReedlineEvent::Edit(cmds) => {
                assert_eq!(cmds.len(), 1);
                match &cmds[0] {
                    EditCommand::InsertChar(c) => assert_eq!(c, expected_char),
                    _ => panic!("Expected InsertChar"),
                }
            }
            _ => panic!("Expected Edit event"),
        }
    }
}

/// Test the full flow: simulate typing characters then check auto-match behavior
#[test]
fn test_auto_match_full_flow_at_end() {
    let state_ref = new_editor_state_ref();
    let rules = create_auto_match_rules();

    // Simulate typing "abc" - update state manually as if these events happened
    {
        let mut state = state_ref.lock().unwrap();
        // After typing 'a'
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
        // After typing 'b'
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        // After typing 'c'
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
        // State should now be: cursor_pos=3, buffer_len=3
        assert_eq!(state.cursor_pos, 3);
        assert_eq!(state.buffer_len, 3);
        assert!(state.cursor_at_end());
    }

    // Now check if the paren rule condition passes
    let paren_rule = &rules[0];
    {
        let state = state_ref.lock().unwrap();
        // cursor_at_end should be true
        assert!(state.cursor_at_end());
        // condition should pass
        assert!(paren_rule.condition.check(&state));
    }
}

/// Test that auto-match is blocked when cursor is not at end
#[test]
fn test_auto_match_blocked_when_not_at_end() {
    let state_ref = new_editor_state_ref();
    let rules = create_auto_match_rules();

    // Simulate typing "abc" then moving left
    {
        let mut state = state_ref.lock().unwrap();
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
        // Move left
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveLeft {
            select: false,
        }]));
        // State should now be: cursor_pos=2, buffer_len=3
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(state.buffer_len, 3);
        assert!(!state.cursor_at_end());
    }

    // Now check if the paren rule condition fails (cursor not at end)
    let paren_rule = &rules[0];
    {
        let state = state_ref.lock().unwrap();
        assert!(!state.cursor_at_end());
        assert!(!paren_rule.condition.check(&state));
    }
}

/// Test that ReedlineEvent::Right is handled in UntilFound when cursor NOT at end
///
/// In reedline, Right arrow binding is:
/// `UntilFound([HistoryHintComplete, MenuRight, Right])`
///
/// When cursor is NOT at buffer end, HistoryHintComplete will fail,
/// so we can safely track the Right movement.
#[test]
fn test_until_found_right_updates_cursor() {
    let mut state = EditorState::new();

    // Simulate typing "abc"
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
    assert_eq!(state.cursor_pos, 3);
    assert_eq!(state.buffer_len, 3);

    // Move left so cursor is NOT at end
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveLeft {
        select: false,
    }]));
    assert_eq!(state.cursor_pos, 2);
    assert!(!state.cursor_at_end());

    // Simulate right arrow key (UntilFound with Right)
    // Since cursor is NOT at end, HistoryHintComplete will fail,
    // so the Right event will succeed and we track it.
    state.update_from_event(&ReedlineEvent::UntilFound(vec![
        ReedlineEvent::HistoryHintComplete,
        ReedlineEvent::MenuRight,
        ReedlineEvent::Right,
    ]));

    // After Right, cursor should be at position 3 (back at end)
    assert_eq!(state.cursor_pos, 3);
    assert!(state.cursor_at_end());
    assert!(!state.uncertain); // State is still certain
}

/// Test that Right arrow at buffer end marks state as uncertain
///
/// When cursor is at buffer end and Right arrow is pressed, HistoryHintComplete
/// might succeed (if a hint is visible), changing the buffer significantly.
/// Since we can't know if a hint was completed, we mark state as uncertain.
#[test]
fn test_until_found_right_at_end_marks_uncertain() {
    let mut state = EditorState::new();

    // Simulate typing "abc" - cursor at end
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
    assert_eq!(state.cursor_pos, 3);
    assert!(state.cursor_at_end());
    assert!(!state.uncertain);

    // Simulate right arrow key while cursor at end
    // HistoryHintComplete might succeed, so state becomes uncertain
    state.update_from_event(&ReedlineEvent::UntilFound(vec![
        ReedlineEvent::HistoryHintComplete,
        ReedlineEvent::MenuRight,
        ReedlineEvent::Right,
    ]));

    // State should be marked as uncertain
    assert!(state.uncertain);
}

/// Test that Right arrow on EMPTY buffer does NOT mark uncertain
///
/// HistoryHintComplete requires min_chars >= 1 to show a hint.
/// On empty buffer, no hint can be shown, so HistoryHintComplete will fail.
/// We should NOT mark uncertain in this case.
#[test]
fn test_until_found_right_on_empty_buffer_not_uncertain() {
    let mut state = EditorState::new();

    // Empty buffer: cursor_pos=0, buffer_len=0
    assert_eq!(state.cursor_pos, 0);
    assert_eq!(state.buffer_len, 0);
    assert!(state.cursor_at_end()); // 0 == 0
    assert!(!state.uncertain);

    // Simulate right arrow key on empty buffer
    state.update_from_event(&ReedlineEvent::UntilFound(vec![
        ReedlineEvent::HistoryHintComplete,
        ReedlineEvent::MenuRight,
        ReedlineEvent::Right,
    ]));

    // State should NOT be uncertain (no hint possible on empty buffer)
    assert!(!state.uncertain);
    // Cursor should still be at 0 (can't move right on empty buffer)
    assert_eq!(state.cursor_pos, 0);
}

/// Regression test: auto-match should work inside braces
///
/// Scenario: `{|}` -> type `(` -> `{(|)}`
/// This tests that the shadow state correctly tracks cursor position
/// inside bracket pairs, allowing nested auto-match to work.
#[test]
fn test_auto_match_inside_braces_regression() {
    let rules = create_auto_match_rules();
    let paren_rule = &rules[0]; // '(' rule

    let mut state = EditorState::new();

    // Type `{` with auto-match: inserts "{}" and moves cursor between
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("{}".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));

    // State: `{|}` - cursor between braces
    assert_eq!(state.buffer, "{}");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 2);
    assert!(!state.cursor_at_end());
    assert!(!state.uncertain);

    // Check that char after cursor is `}`
    assert_eq!(state.char_after_cursor(), Some('}'));

    // The condition for `(` auto-match should pass
    // (cursor is before closing char `}`)
    assert!(paren_rule.condition.check(&state));

    // Simulate typing `(` with auto-match
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));

    // Result: `{(|)}`
    assert_eq!(state.buffer, "{()}");
    assert_eq!(state.cursor_pos, 2); // Between the parens
}

/// Test that ReedlineEvent::Left is handled in UntilFound
///
/// Note: In actual reedline, Left arrow binding is:
/// `UntilFound([MenuLeft, Left])` - NO HistoryHintComplete
/// (HistoryHintComplete is only in Right arrow binding)
#[test]
fn test_until_found_left_updates_cursor() {
    let mut state = EditorState::new();

    // Simulate typing "abc"
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
    assert_eq!(state.cursor_pos, 3);
    assert!(state.cursor_at_end());

    // Simulate left arrow key (UntilFound with Left)
    // This matches actual reedline binding: UntilFound([MenuLeft, Left])
    state.update_from_event(&ReedlineEvent::UntilFound(vec![
        ReedlineEvent::MenuLeft,
        ReedlineEvent::Left,
    ]));

    // After Left, cursor should be at position 2
    assert_eq!(state.cursor_pos, 2);
    assert!(!state.cursor_at_end());
}

/// Test direct ReedlineEvent::Right handling
#[test]
fn test_right_event_updates_cursor() {
    let mut state = EditorState::new();
    state.buffer_len = 5;
    state.cursor_pos = 2;

    state.update_from_event(&ReedlineEvent::Right);
    assert_eq!(state.cursor_pos, 3);

    // At end, should not go past
    state.cursor_pos = 5;
    state.update_from_event(&ReedlineEvent::Right);
    assert_eq!(state.cursor_pos, 5);
}

/// Test direct ReedlineEvent::Left handling
#[test]
fn test_left_event_updates_cursor() {
    let mut state = EditorState::new();
    state.buffer_len = 5;
    state.cursor_pos = 2;

    state.update_from_event(&ReedlineEvent::Left);
    assert_eq!(state.cursor_pos, 1);

    // At beginning, should not go negative
    state.cursor_pos = 0;
    state.update_from_event(&ReedlineEvent::Left);
    assert_eq!(state.cursor_pos, 0);
}
