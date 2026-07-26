use super::super::*;

// ===== Tests for bracket delete rules =====

#[test]
fn test_bracket_delete_rules_created() {
    let rules = create_bracket_delete_rules();
    assert_eq!(rules.len(), 1);
}

#[test]
fn test_bracket_delete_rule_matches_backspace() {
    let rules = create_bracket_delete_rules();
    let rule = &rules[0];

    // Should match a single Backspace command
    let backspace_event = ReedlineEvent::Edit(vec![EditCommand::Backspace]);
    assert!((rule.match_event)(&backspace_event));

    // Should not match other events
    let delete_event = ReedlineEvent::Edit(vec![EditCommand::Delete]);
    assert!(!(rule.match_event)(&delete_event));

    // Should not match multiple commands
    let multiple_event = ReedlineEvent::Edit(vec![EditCommand::Backspace, EditCommand::Delete]);
    assert!(!(rule.match_event)(&multiple_event));
}

#[test]
fn test_bracket_delete_condition_not_inside_pair() {
    let rules = create_bracket_delete_rules();
    let rule = &rules[0];

    // Not inside pair - condition should return true (keep original backspace)
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 2;
    assert!(rule.condition.check(&state));
}

#[test]
fn test_bracket_delete_condition_inside_pair() {
    let rules = create_bracket_delete_rules();
    let rule = &rules[0];

    // Inside empty pair - condition should return false (use fallback)
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;
    assert!(!rule.condition.check(&state));
}

#[test]
fn test_bracket_delete_fallback_event() {
    let rules = create_bracket_delete_rules();
    let rule = &rules[0];

    // Fallback should be Backspace + Delete
    match &rule.fallback_event {
        ReedlineEvent::Edit(cmds) => {
            assert_eq!(cmds.len(), 2);
            assert!(matches!(&cmds[0], EditCommand::Backspace));
            assert!(matches!(&cmds[1], EditCommand::Delete));
        }
        _ => panic!("Expected Edit event"),
    }
}

#[test]
fn test_bracket_delete_full_flow() {
    let mut state = EditorState::new();

    // Type '(' with auto-match: inserts "()" and moves cursor between
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));
    assert_eq!(state.buffer, "()");
    assert_eq!(state.cursor_pos, 1);
    assert!(state.is_inside_empty_pair());

    // Simulate the bracket delete rule's fallback event: Backspace + Delete
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::Backspace,
        EditCommand::Delete,
    ]));

    // Both brackets should be deleted
    assert_eq!(state.buffer, "");
    assert_eq!(state.cursor_pos, 0);
    assert_eq!(state.buffer_len, 0);
}

#[test]
fn test_bracket_delete_all_pair_types() {
    let pairs = [
        ("()", 1),
        ("[]", 1),
        ("{}", 1),
        (r#""""#, 1),
        ("''", 1),
        ("``", 1),
    ];

    for (pair, cursor_pos) in pairs {
        let mut state = EditorState::new();
        state.buffer = pair.to_string();
        state.buffer_len = 2;
        state.cursor_pos = cursor_pos;

        assert!(
            state.is_inside_empty_pair(),
            "Expected to be inside empty pair for: {}",
            pair
        );

        // Apply the fallback event
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::Backspace,
            EditCommand::Delete,
        ]));

        assert_eq!(
            state.buffer, "",
            "Buffer should be empty after deleting {}",
            pair
        );
    }
}

#[test]
fn test_bracket_delete_uncertain_state() {
    let rules = create_bracket_delete_rules();
    let rule = &rules[0];

    // When state is uncertain, condition should return true (keep original backspace)
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;
    state.uncertain = true;

    // is_inside_empty_pair returns false when uncertain
    assert!(!state.is_inside_empty_pair());
    // So NotInsideEmptyPair returns true
    assert!(rule.condition.check(&state));
}

// ===== Tests for backspace after newline inside brackets =====

/// Test: backspace after newline inside brackets should NOT delete the closing bracket.
///
/// Bug scenario:
/// 1. Type '(' -> auto-match gives "()" with cursor between
/// 2. Press Enter -> buffer becomes "(\n)" with cursor after newline
/// 3. Press Backspace -> should delete only newline, NOT the closing bracket
///
/// The bracket delete rule should NOT trigger because char_before_cursor is '\n', not '('.
#[test]
fn test_backspace_after_newline_not_inside_empty_pair() {
    let mut state = EditorState::new();

    // Simulate typing '(' with auto-match
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));
    assert_eq!(state.buffer, "()");
    assert_eq!(state.cursor_pos, 1);
    assert!(state.is_inside_empty_pair());

    // Simulate pressing Enter (InsertNewline)
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertNewline]));
    assert_eq!(state.buffer, "(\n)");
    assert_eq!(state.cursor_pos, 2);

    // After newline, we should NOT be inside an empty pair
    // because char_before_cursor is '\n', not '('
    assert!(!state.is_inside_empty_pair());

    // Verify the characters
    assert_eq!(state.char_before_cursor(), Some('\n'));
    assert_eq!(state.char_after_cursor(), Some(')'));
}

/// Test: bracket delete rule condition after newline inside brackets.
///
/// NotInsideEmptyPair should return true (keep original Backspace)
/// because we're not inside an empty pair after the newline.
#[test]
fn test_bracket_delete_condition_after_newline() {
    let rules = create_bracket_delete_rules();
    let rule = &rules[0];

    let mut state = EditorState::new();
    state.buffer = "(\n)".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 2; // After '\n', before ')'

    // is_inside_empty_pair should be false
    assert!(!state.is_inside_empty_pair());

    // NotInsideEmptyPair should return true (keep original Backspace)
    assert!(rule.condition.check(&state));
}

/// Test: backspace after newline should only delete the newline.
#[test]
fn test_backspace_after_newline_deletes_only_newline() {
    let mut state = EditorState::new();
    state.buffer = "(\n)".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 2;

    // Apply single Backspace (not Backspace + Delete)
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));

    // Should only delete the newline
    assert_eq!(state.buffer, "()");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 2);
}
