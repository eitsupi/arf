use super::super::*;
use super::create_test_targets;

#[test]
fn test_import_entries_to_sqlite() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    // Create test entries (R mode)
    let entries = vec![
        ImportEntry {
            command: "library(ggplot2)".to_string(),
            timestamp: Some(Utc::now()),
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "print('hello')".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
    ];

    let result = import_entries(&mut targets, entries, None, false).unwrap();

    assert_eq!(result.r_imported, 2);
    assert_eq!(result.shell_imported, 0);
    assert_eq!(result.skipped, 0);
    assert!(result.warnings.is_empty());

    // Verify entries were imported to R history
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 2);

    let commands: Vec<&str> = items.iter().map(|i| i.command_line.as_str()).collect();
    assert!(commands.contains(&"library(ggplot2)"));
    assert!(commands.contains(&"print('hello')"));
}

#[test]
fn test_import_entries_routes_shell_mode() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    // Create mixed mode entries
    let entries = vec![
        ImportEntry {
            command: "library(dplyr)".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "ls -la".to_string(),
            timestamp: None,
            mode: Some("shell".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "pwd".to_string(),
            timestamp: None,
            mode: Some("shell".to_string()),
            metadata: None,
        },
    ];

    let result = import_entries(&mut targets, entries, None, false).unwrap();

    assert_eq!(result.r_imported, 1);
    assert_eq!(result.shell_imported, 2);
    assert_eq!(result.skipped, 0);

    // Verify R history
    let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let r_items = targets.r_history.search(r_query).unwrap();
    assert_eq!(r_items.len(), 1);
    assert_eq!(r_items[0].command_line, "library(dplyr)");

    // Verify shell history
    let shell_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let shell_items = targets.shell_history.search(shell_query).unwrap();
    assert_eq!(shell_items.len(), 2);
    let shell_commands: Vec<&str> = shell_items
        .iter()
        .map(|i| i.command_line.as_str())
        .collect();
    assert!(shell_commands.contains(&"ls -la"));
    assert!(shell_commands.contains(&"pwd"));
}

#[test]
fn test_import_entries_dry_run() {
    // Create mixed mode entries
    let entries = vec![
        ImportEntry {
            command: "test_r".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "test_shell".to_string(),
            timestamp: None,
            mode: Some("shell".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "unknown_mode".to_string(),
            timestamp: None,
            mode: Some("python".to_string()), // Unknown mode
            metadata: None,
        },
        ImportEntry {
            command: "   ".to_string(), // Whitespace-only, should be skipped
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
    ];

    // import_entries_dry_run doesn't need database handles
    let result = import_entries_dry_run(&entries, None, None);

    assert_eq!(result.r_imported, 1);
    assert_eq!(result.shell_imported, 1);
    assert_eq!(result.skipped, 2); // unknown mode + whitespace-only
    assert_eq!(result.warnings.len(), 1); // warning for unknown mode
    assert!(result.warnings[0].contains("python"));
}

#[test]
fn test_import_entries_skips_empty() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let entries = vec![
        ImportEntry {
            command: "valid".to_string(),
            timestamp: None,
            mode: None,
            metadata: None,
        },
        ImportEntry {
            command: "   ".to_string(), // Whitespace only - should be skipped
            timestamp: None,
            mode: None,
            metadata: None,
        },
        ImportEntry {
            command: "".to_string(), // Empty - should be skipped
            timestamp: None,
            mode: None,
            metadata: None,
        },
    ];

    let result = import_entries(&mut targets, entries, None, false).unwrap();

    assert_eq!(result.r_imported, 1); // "valid" goes to R (mode: None)
    assert_eq!(result.shell_imported, 0);
    assert_eq!(result.skipped, 2);
}

#[test]
fn test_import_entries_skips_unknown_modes() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let entries = vec![
        ImportEntry {
            command: "valid_r".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "valid_shell".to_string(),
            timestamp: None,
            mode: Some("shell".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "unknown_mode_cmd".to_string(),
            timestamp: None,
            mode: Some("python".to_string()), // Unknown mode
            metadata: None,
        },
        ImportEntry {
            command: "another_unknown".to_string(),
            timestamp: None,
            mode: Some("jupyter".to_string()), // Unknown mode
            metadata: None,
        },
    ];

    let result = import_entries(&mut targets, entries, None, false).unwrap();

    assert_eq!(result.r_imported, 1);
    assert_eq!(result.shell_imported, 1);
    assert_eq!(result.skipped, 2);
    assert_eq!(result.warnings.len(), 2);
    assert!(result.warnings[0].contains("python"));
    assert!(result.warnings[1].contains("jupyter"));
}

#[test]
fn test_import_entries_handles_browse_mode() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let entries = vec![
        ImportEntry {
            command: "n".to_string(), // debug step
            timestamp: None,
            mode: Some("browse".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "c".to_string(), // continue
            timestamp: None,
            mode: Some("browse".to_string()),
            metadata: None,
        },
    ];

    let result = import_entries(&mut targets, entries, None, false).unwrap();

    // browse mode should go to R database
    assert_eq!(result.r_imported, 2);
    assert_eq!(result.shell_imported, 0);
    assert_eq!(result.skipped, 0);

    // Verify entries are in R history
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 2);
}

#[test]
fn test_import_entries_none_mode_goes_to_r_database() {
    // Entries with mode=None should go to R database (default behavior)
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let entries = vec![ImportEntry {
        command: "summary(mtcars)".to_string(),
        timestamp: None,
        mode: None, // No mode specified
        metadata: None,
    }];

    let result = import_entries(&mut targets, entries, None, false).unwrap();
    assert_eq!(result.r_imported, 1);
    assert_eq!(result.shell_imported, 0);

    // Verify it's in R history
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].command_line, "summary(mtcars)");
}

#[test]
fn test_import_entries_with_hostname_override() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let entries = vec![
        ImportEntry {
            command: "library(dplyr)".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "ls -la".to_string(),
            timestamp: None,
            mode: Some("shell".to_string()),
            metadata: None,
        },
    ];

    // Import with custom hostname
    let result = import_entries(&mut targets, entries, Some("radian-import"), false).unwrap();

    assert_eq!(result.r_imported, 1);
    assert_eq!(result.shell_imported, 1);

    // Verify R history has the custom hostname
    let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let r_items = targets.r_history.search(r_query).unwrap();
    assert_eq!(r_items.len(), 1);
    assert_eq!(r_items[0].hostname, Some("radian-import".to_string()));

    // Verify shell history also has the custom hostname
    let shell_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let shell_items = targets.shell_history.search(shell_query).unwrap();
    assert_eq!(shell_items.len(), 1);
    assert_eq!(shell_items[0].hostname, Some("radian-import".to_string()));
}
