use super::super::*;

// ===== Tests for buffer content tracking =====

#[test]
fn test_char_before_cursor() {
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 2;

    assert_eq!(state.char_before_cursor(), Some('b'));

    // At beginning, should return None
    state.cursor_pos = 0;
    assert_eq!(state.char_before_cursor(), None);

    // At end
    state.cursor_pos = 3;
    assert_eq!(state.char_before_cursor(), Some('c'));
}

#[test]
fn test_char_after_cursor() {
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 1;

    assert_eq!(state.char_after_cursor(), Some('b'));

    // At end, should return None
    state.cursor_pos = 3;
    assert_eq!(state.char_after_cursor(), None);

    // At beginning
    state.cursor_pos = 0;
    assert_eq!(state.char_after_cursor(), Some('a'));
}

#[test]
fn test_char_methods_uncertain() {
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 1;
    state.uncertain = true;

    // When uncertain, both should return None
    assert_eq!(state.char_before_cursor(), None);
    assert_eq!(state.char_after_cursor(), None);
}

#[test]
fn test_is_inside_empty_pair_parens() {
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1; // Between ( and )

    assert!(state.is_inside_empty_pair());
}

#[test]
fn test_is_inside_empty_pair_brackets() {
    let mut state = EditorState::new();
    state.buffer = "[]".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;

    assert!(state.is_inside_empty_pair());
}

#[test]
fn test_is_inside_empty_pair_braces() {
    let mut state = EditorState::new();
    state.buffer = "{}".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;

    assert!(state.is_inside_empty_pair());
}

#[test]
fn test_is_inside_empty_pair_quotes() {
    let mut state = EditorState::new();

    // Double quotes
    state.buffer = r#""""#.to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;
    assert!(state.is_inside_empty_pair());

    // Single quotes
    state.buffer = "''".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;
    assert!(state.is_inside_empty_pair());

    // Backticks
    state.buffer = "``".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;
    assert!(state.is_inside_empty_pair());
}

#[test]
fn test_is_inside_empty_pair_not_empty() {
    let mut state = EditorState::new();
    state.buffer = "(x)".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 1; // Before 'x'

    // Not an empty pair - there's content inside
    assert!(!state.is_inside_empty_pair());
}

#[test]
fn test_is_inside_empty_pair_mismatched() {
    let mut state = EditorState::new();
    state.buffer = "(]".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;

    // Mismatched pair
    assert!(!state.is_inside_empty_pair());
}

#[test]
fn test_is_inside_empty_pair_uncertain() {
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1;
    state.uncertain = true;

    // When uncertain, should return false for safety
    assert!(!state.is_inside_empty_pair());
}

#[test]
fn test_is_inside_empty_pair_edge_positions() {
    let mut state = EditorState::new();
    state.buffer = "()".to_string();
    state.buffer_len = 2;

    // At beginning (before '(')
    state.cursor_pos = 0;
    assert!(!state.is_inside_empty_pair());

    // At end (after ')')
    state.cursor_pos = 2;
    assert!(!state.is_inside_empty_pair());
}

#[test]
fn test_reset_clears_buffer() {
    let mut state = EditorState::new();
    state.buffer = "test".to_string();
    state.buffer_len = 4;
    state.cursor_pos = 2;
    state.uncertain = true;

    state.reset();

    assert!(state.buffer.is_empty());
    assert_eq!(state.buffer_len, 0);
    assert_eq!(state.cursor_pos, 0);
    assert!(!state.uncertain);
}

// ===== Tests for buffer content updates via events =====

#[test]
fn test_insert_char_updates_buffer() {
    let mut state = EditorState::new();

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
    assert_eq!(state.buffer, "a");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 1);

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
    assert_eq!(state.buffer, "ab");
    assert_eq!(state.cursor_pos, 2);
    assert_eq!(state.buffer_len, 2);
}

#[test]
fn test_insert_string_updates_buffer() {
    let mut state = EditorState::new();

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertString(
        "hello".to_string(),
    )]));
    assert_eq!(state.buffer, "hello");
    assert_eq!(state.cursor_pos, 5);
    assert_eq!(state.buffer_len, 5);
}

#[test]
fn test_insert_char_in_middle() {
    let mut state = EditorState::new();
    state.buffer = "ac".to_string();
    state.buffer_len = 2;
    state.cursor_pos = 1; // After 'a'

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
    assert_eq!(state.buffer, "abc");
    assert_eq!(state.cursor_pos, 2);
    assert_eq!(state.buffer_len, 3);
}

#[test]
fn test_backspace_updates_buffer() {
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 3;

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
    assert_eq!(state.buffer, "ab");
    assert_eq!(state.cursor_pos, 2);
    assert_eq!(state.buffer_len, 2);
}

#[test]
fn test_backspace_in_middle() {
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 2; // After 'b'

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
    assert_eq!(state.buffer, "ac");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 2);
}

#[test]
fn test_delete_updates_buffer() {
    let mut state = EditorState::new();
    state.buffer = "abc".to_string();
    state.buffer_len = 3;
    state.cursor_pos = 1; // After 'a'

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Delete]));
    assert_eq!(state.buffer, "ac");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 2);
}

#[test]
fn test_cut_from_start_updates_buffer() {
    let mut state = EditorState::new();
    state.buffer = "hello world".to_string();
    state.buffer_len = 11;
    state.cursor_pos = 6; // After "hello "

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::CutFromStart]));
    assert_eq!(state.buffer, "world");
    assert_eq!(state.cursor_pos, 0);
    assert_eq!(state.buffer_len, 5);
}

#[test]
fn test_cut_to_end_updates_buffer() {
    let mut state = EditorState::new();
    state.buffer = "hello world".to_string();
    state.buffer_len = 11;
    state.cursor_pos = 5; // After "hello"

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::CutToEnd]));
    assert_eq!(state.buffer, "hello");
    assert_eq!(state.cursor_pos, 5);
    assert_eq!(state.buffer_len, 5);
}

#[test]
fn test_history_navigation_sets_uncertain() {
    let mut state = EditorState::new();
    state.buffer = "test".to_string();
    state.buffer_len = 4;
    state.cursor_pos = 4;
    assert!(!state.uncertain);

    state.update_from_event(&ReedlineEvent::Up);
    assert!(state.uncertain);
}

#[test]
fn test_unicode_insert_and_delete() {
    let mut state = EditorState::new();

    // Insert Unicode characters
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('日')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('本')]));
    assert_eq!(state.buffer, "日本");
    assert_eq!(state.cursor_pos, 2);
    assert_eq!(state.buffer_len, 2);

    // Backspace removes one character
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
    assert_eq!(state.buffer, "日");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 1);
}

#[test]
fn test_auto_pair_simulation() {
    let mut state = EditorState::new();

    // Simulate typing '(' which triggers auto-match: InsertString("()") + MoveLeft
    state.update_from_event(&ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]));
    assert_eq!(state.buffer, "()");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 2);

    // Now cursor is inside empty pair
    assert!(state.is_inside_empty_pair());

    // Backspace should delete '('
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
    assert_eq!(state.buffer, ")");
    assert_eq!(state.cursor_pos, 0);
    assert_eq!(state.buffer_len, 1);
}
