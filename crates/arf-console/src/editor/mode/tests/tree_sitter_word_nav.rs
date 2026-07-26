use super::super::*;

// ===== Tests for tree-sitter word navigation =====

/// Test: create_move_event generates correct left movement
#[test]
fn test_create_move_event_left() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    // Moving from position 5 to position 2 should generate 3 MoveLeft commands
    let event = ConditionalEditMode::<Emacs>::create_move_event(5, 2, false);

    match event {
        ReedlineEvent::Edit(commands) => {
            assert_eq!(commands.len(), 3);
            for cmd in commands {
                assert!(matches!(cmd, EditCommand::MoveLeft { select: false }));
            }
        }
        _ => panic!("Expected Edit event with MoveLeft commands"),
    }
}

/// Test: create_move_event generates correct right movement
#[test]
fn test_create_move_event_right() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    // Moving from position 2 to position 5 should generate 3 MoveRight commands
    let event = ConditionalEditMode::<Emacs>::create_move_event(2, 5, false);

    match event {
        ReedlineEvent::Edit(commands) => {
            assert_eq!(commands.len(), 3);
            for cmd in commands {
                assert!(matches!(cmd, EditCommand::MoveRight { select: false }));
            }
        }
        _ => panic!("Expected Edit event with MoveRight commands"),
    }
}

/// Test: create_move_event with select=true
#[test]
fn test_create_move_event_with_selection() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    let event = ConditionalEditMode::<Emacs>::create_move_event(2, 5, true);

    match event {
        ReedlineEvent::Edit(commands) => {
            assert_eq!(commands.len(), 3);
            for cmd in commands {
                assert!(matches!(cmd, EditCommand::MoveRight { select: true }));
            }
        }
        _ => panic!("Expected Edit event with MoveRight commands"),
    }
}

/// Test: create_move_event returns None when positions are equal
#[test]
fn test_create_move_event_same_position() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    let event = ConditionalEditMode::<Emacs>::create_move_event(5, 5, false);
    assert!(matches!(event, ReedlineEvent::None));
}

/// Test: handle_tree_sitter_word_nav transforms MoveWordRight for pipe operator
#[test]
fn test_tree_sitter_word_nav_pipe_operator() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    let state_ref = new_editor_state_ref();

    // Set up state: "x |> filter()"
    //                  ^-- cursor at position 2 (before |>)
    {
        let mut state = state_ref.lock().unwrap();
        state.buffer = "x |> filter()".to_string();
        state.buffer_len = 13;
        state.cursor_pos = 2;
        state.uncertain = false;
    }

    let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
        .with_tree_sitter_word_nav(true);

    // Test MoveWordRight - should jump over "|>" as a single token
    let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]);
    let result = conditional.handle_tree_sitter_word_nav(&event);

    assert!(result.is_some());
    let result_event = result.unwrap();

    // Should generate MoveRight commands to move from position 2 to 4 (after "|>")
    match result_event {
        ReedlineEvent::Edit(commands) => {
            assert_eq!(commands.len(), 2); // Move 2 positions right
            for cmd in commands {
                assert!(matches!(cmd, EditCommand::MoveRight { select: false }));
            }
        }
        _ => panic!("Expected Edit event"),
    }
}

/// Test: handle_tree_sitter_word_nav transforms MoveWordLeft for assignment
#[test]
fn test_tree_sitter_word_nav_assignment_left() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    let state_ref = new_editor_state_ref();

    // Set up state: "x <- 42"
    //                   ^-- cursor at position 4 (after "<-")
    {
        let mut state = state_ref.lock().unwrap();
        state.buffer = "x <- 42".to_string();
        state.buffer_len = 7;
        state.cursor_pos = 4;
        state.uncertain = false;
    }

    let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
        .with_tree_sitter_word_nav(true);

    // Test MoveWordLeft - should jump to start of "<-"
    let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordLeft { select: false }]);
    let result = conditional.handle_tree_sitter_word_nav(&event);

    assert!(result.is_some());
    let result_event = result.unwrap();

    // Should generate MoveLeft commands to move from position 4 to 2 (start of "<-")
    match result_event {
        ReedlineEvent::Edit(commands) => {
            assert_eq!(commands.len(), 2); // Move 2 positions left
            for cmd in commands {
                assert!(matches!(cmd, EditCommand::MoveLeft { select: false }));
            }
        }
        _ => panic!("Expected Edit event"),
    }
}

/// Test: handle_tree_sitter_word_nav is disabled when tree_sitter_word_nav=false
#[test]
fn test_tree_sitter_word_nav_disabled() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    let state_ref = new_editor_state_ref();

    {
        let mut state = state_ref.lock().unwrap();
        state.buffer = "x |> filter()".to_string();
        state.buffer_len = 13;
        state.cursor_pos = 2;
        state.uncertain = false;
    }

    // tree-sitter word nav is disabled by default
    let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone());

    let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]);
    let result = conditional.handle_tree_sitter_word_nav(&event);

    // Should return None when disabled
    assert!(result.is_none());
}

/// Test: handle_tree_sitter_word_nav returns None when state is uncertain
#[test]
fn test_tree_sitter_word_nav_uncertain_state() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    let state_ref = new_editor_state_ref();

    {
        let mut state = state_ref.lock().unwrap();
        state.buffer = "x |> filter()".to_string();
        state.buffer_len = 13;
        state.cursor_pos = 2;
        state.uncertain = true; // State is uncertain
    }

    let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
        .with_tree_sitter_word_nav(true);

    let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]);
    let result = conditional.handle_tree_sitter_word_nav(&event);

    // Should return None when state is uncertain (falls back to default behavior)
    assert!(result.is_none());
}

/// Test: handle_tree_sitter_word_nav handles UntilFound with word movement
#[test]
fn test_tree_sitter_word_nav_until_found() {
    use super::super::ConditionalEditMode;
    use reedline::Emacs;

    let state_ref = new_editor_state_ref();

    {
        let mut state = state_ref.lock().unwrap();
        state.buffer = "x |> filter()".to_string();
        state.buffer_len = 13;
        state.cursor_pos = 2;
        state.uncertain = false;
    }

    let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
        .with_tree_sitter_word_nav(true);

    // Simulate Ctrl+Right binding: UntilFound with HistoryHintWordComplete and MoveWordRight
    let event = ReedlineEvent::UntilFound(vec![
        ReedlineEvent::HistoryHintWordComplete,
        ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]),
    ]);
    let result = conditional.handle_tree_sitter_word_nav(&event);

    assert!(result.is_some());
    let result_event = result.unwrap();

    // Should be UntilFound with replaced MoveWordRight
    match result_event {
        ReedlineEvent::UntilFound(events) => {
            assert_eq!(events.len(), 2);
            // First event should still be HistoryHintWordComplete
            assert!(matches!(events[0], ReedlineEvent::HistoryHintWordComplete));
            // Second event should be transformed to MoveRight commands
            match &events[1] {
                ReedlineEvent::Edit(commands) => {
                    assert_eq!(commands.len(), 2);
                }
                _ => panic!("Expected Edit event"),
            }
        }
        _ => panic!("Expected UntilFound event"),
    }
}
