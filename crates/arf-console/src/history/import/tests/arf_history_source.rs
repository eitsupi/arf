use super::super::*;
use super::create_test_targets;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_parse_arf_history_not_found() {
    use tempfile::TempDir;

    // Use TempDir to guarantee a non-existent file path
    let temp_dir = TempDir::new().unwrap();
    let missing_path = temp_dir.path().join("nonexistent.db");

    let result = parse_arf_history(&missing_path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not found"));
}

#[test]
fn test_parse_arf_history_infers_mode_from_filename() {
    use reedline::History;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create an R history database
    let r_path = temp_dir.path().join("r.db");
    let mut r_db = SqliteBackedHistory::with_file(r_path.clone(), None, None).unwrap();
    r_db.save(HistoryItem {
        id: None,
        command_line: "summary(iris)".to_string(),
        start_timestamp: None,
        session_id: None,
        hostname: None,
        cwd: None,
        duration: None,
        exit_status: None,
        more_info: None,
    })
    .unwrap();
    drop(r_db); // Close the database

    // Create a shell history database
    let shell_path = temp_dir.path().join("shell.db");
    let mut shell_db = SqliteBackedHistory::with_file(shell_path.clone(), None, None).unwrap();
    shell_db
        .save(HistoryItem {
            id: None,
            command_line: "ls -la".to_string(),
            start_timestamp: None,
            session_id: None,
            hostname: None,
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: None,
        })
        .unwrap();
    drop(shell_db);

    // Parse R history - should have mode "r"
    let r_entries = parse_arf_history(&r_path).unwrap();
    assert_eq!(r_entries.len(), 1);
    assert_eq!(r_entries[0].mode, Some("r".to_string()));

    // Parse shell history - should have mode "shell"
    let shell_entries = parse_arf_history(&shell_path).unwrap();
    assert_eq!(shell_entries.len(), 1);
    assert_eq!(shell_entries[0].mode, Some("shell".to_string()));
}

#[test]
fn test_arf_shell_to_shell_import() {
    use reedline::History;
    use tempfile::TempDir;

    // Use separate directories for source and target to avoid conflicts
    let source_dir = TempDir::new().unwrap();
    let target_dir = TempDir::new().unwrap();

    // Create a source shell history database
    let source_path = source_dir.path().join("old_shell.db");
    let mut source_db = SqliteBackedHistory::with_file(source_path.clone(), None, None).unwrap();
    source_db
        .save(HistoryItem {
            id: None,
            command_line: "git status".to_string(),
            start_timestamp: Some(Utc::now()),
            session_id: None,
            hostname: None,
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: None,
        })
        .unwrap();
    drop(source_db);

    // Note: filename doesn't end with "shell.db" so it will be treated as R
    // This tests that only exact "shell.db" filename triggers shell mode
    let entries = parse_arf_history(&source_path).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].mode, Some("r".to_string())); // Not shell!

    // Now test with exact "shell.db" filename
    let shell_source_path = source_dir.path().join("shell.db");
    std::fs::copy(&source_path, &shell_source_path).unwrap();

    let shell_entries = parse_arf_history(&shell_source_path).unwrap();
    assert_eq!(shell_entries.len(), 1);
    assert_eq!(shell_entries[0].mode, Some("shell".to_string()));

    // Import to target databases (in separate directory)
    let mut targets = create_test_targets(&target_dir);
    let result = import_entries(&mut targets, shell_entries.entries, None, false).unwrap();

    assert_eq!(result.r_imported, 0);
    assert_eq!(result.shell_imported, 1);

    // Verify it went to shell database
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.shell_history.search(query).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].command_line, "git status");
}

#[test]
fn test_end_to_end_radian_import_with_shell() {
    use tempfile::TempDir;

    // Create a radian history file with both R and shell commands
    let mut source_file = NamedTempFile::new().unwrap();
    writeln!(source_file, "# time: 2024-03-15 09:00:00 UTC").unwrap();
    writeln!(source_file, "# mode: r").unwrap();
    writeln!(source_file, "+summary(mtcars)").unwrap();
    writeln!(source_file).unwrap();
    writeln!(source_file, "# time: 2024-03-15 09:01:00 UTC").unwrap();
    writeln!(source_file, "# mode: shell").unwrap();
    writeln!(source_file, "+git status").unwrap();
    writeln!(source_file).unwrap();
    writeln!(source_file, "# time: 2024-03-15 09:02:00 UTC").unwrap();
    writeln!(source_file, "# mode: r").unwrap();
    writeln!(source_file, "+plot(mtcars$mpg, mtcars$hp)").unwrap();

    // Parse the radian history
    let entries = parse_radian_history(source_file.path()).unwrap();
    assert_eq!(entries.len(), 3);

    // Import to SQLite
    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let result = import_entries(&mut targets, entries.entries, None, false).unwrap();
    assert_eq!(result.r_imported, 2);
    assert_eq!(result.shell_imported, 1);

    // Verify R history
    let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let r_items = targets.r_history.search(r_query).unwrap();
    assert_eq!(r_items.len(), 2);
    // Check timestamps were preserved
    assert!(r_items.iter().all(|i| i.start_timestamp.is_some()));

    // Verify shell history
    let shell_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let shell_items = targets.shell_history.search(shell_query).unwrap();
    assert_eq!(shell_items.len(), 1);
    assert_eq!(shell_items[0].command_line, "git status");
    assert!(shell_items[0].start_timestamp.is_some());
}

#[test]
fn test_arf_history_preserves_unknown_metadata_and_imports_it() {
    use tempfile::TempDir;

    let source_dir = TempDir::new().unwrap();
    let target_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("r.db");
    let mut source_db = SqliteBackedHistory::with_file(source_path.clone(), None, None).unwrap();
    let metadata: HistoryExtraInfo =
        serde_json::from_str(r#"{"meta_command":true,"future_field":{"value":42}}"#).unwrap();
    source_db
        .save_with_extra(HistoryItem {
            id: None,
            command_line: "future command".to_string(),
            start_timestamp: None,
            session_id: None,
            hostname: None,
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: Some(metadata.clone()),
        })
        .unwrap();
    drop(source_db);

    let parsed = parse_arf_history(&source_path).unwrap();
    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!(parsed.entries[0].metadata, Some(metadata.clone()));

    let mut targets = create_test_targets(&target_dir);
    let result = import_entries(&mut targets, parsed.entries, None, false).unwrap();
    assert_eq!(result.r_imported, 1);
    let target = targets
        .r_history
        .load_with_metadata(reedline::HistoryItemId::new(1))
        .unwrap();
    assert_eq!(target.more_info, Some(metadata));
}

#[test]
fn test_arf_history_malformed_metadata_degrades_per_row() {
    use reedline::History;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let source_path = temp_dir.path().join("r.db");
    let mut source_db = SqliteBackedHistory::with_file(source_path.clone(), None, None).unwrap();
    for command in ["bad json", "not an object", "healthy"] {
        source_db
            .save(HistoryItem::from_command_line(command))
            .unwrap();
    }
    drop(source_db);

    let db = rusqlite::Connection::open(&source_path).unwrap();
    db.execute(
        "UPDATE history SET more_info = ? WHERE command_line = ?",
        rusqlite::params![r#"{"unterminated"#, "bad json"],
    )
    .unwrap();
    db.execute(
        "UPDATE history SET more_info = ? WHERE command_line = ?",
        rusqlite::params![r#"["not an object"]"#, "not an object"],
    )
    .unwrap();
    drop(db);

    let parsed = parse_arf_history(&source_path).unwrap();
    assert_eq!(parsed.entries.len(), 3);
    assert!(parsed.entries.iter().all(|entry| entry.metadata.is_none()));
    assert_eq!(parsed.warnings.len(), 2);
    assert!(
        parsed
            .warnings
            .iter()
            .all(|warning| warning.contains(source_path.to_string_lossy().as_ref()))
    );
}
