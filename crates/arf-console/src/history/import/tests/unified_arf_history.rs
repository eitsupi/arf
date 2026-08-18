use super::super::*;
use super::create_test_targets;

#[test]
fn test_parse_unified_arf_history_basic() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let unified_path = temp_dir.path().join("export.db");

    // Create a unified export file with r and shell tables
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    db.execute(
            "CREATE TABLE r (id INTEGER PRIMARY KEY, command_line TEXT NOT NULL, start_timestamp INTEGER)",
            [],
        )
        .unwrap();
    db.execute(
            "CREATE TABLE shell (id INTEGER PRIMARY KEY, command_line TEXT NOT NULL, start_timestamp INTEGER)",
            [],
        )
        .unwrap();

    db.execute(
        "INSERT INTO r (command_line, start_timestamp) VALUES ('library(dplyr)', 1705315800000)",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO r (command_line, start_timestamp) VALUES ('print(1)', NULL)",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO shell (command_line, start_timestamp) VALUES ('ls -la', 1705315860000)",
        [],
    )
    .unwrap();
    drop(db);

    // Parse the unified file
    let entries = parse_unified_arf_history(&unified_path, "r", "shell").unwrap();

    assert_eq!(entries.len(), 3);

    // R entries
    let r_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.mode.as_deref() == Some("r"))
        .collect();
    assert_eq!(r_entries.len(), 2);
    assert_eq!(r_entries[0].command, "library(dplyr)");
    assert!(r_entries[0].timestamp.is_some());
    assert_eq!(r_entries[1].command, "print(1)");
    assert!(r_entries[1].timestamp.is_none());

    // Shell entries
    let shell_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.mode.as_deref() == Some("shell"))
        .collect();
    assert_eq!(shell_entries.len(), 1);
    assert_eq!(shell_entries[0].command, "ls -la");
}

#[test]
fn test_unified_export_preserves_metadata_through_import() {
    use tempfile::TempDir;

    let source_dir = TempDir::new().unwrap();
    let target_dir = TempDir::new().unwrap();
    let unified_path = source_dir.path().join("export.db");
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    db.execute(
        r#"CREATE TABLE r (
            id INTEGER PRIMARY KEY,
            command_line TEXT NOT NULL,
            start_timestamp INTEGER,
            more_info TEXT
        )"#,
        [],
    )
    .unwrap();
    let metadata = r#"{"future_field":{"value":42}}"#;
    db.execute(
        "INSERT INTO r (command_line, more_info) VALUES (?, ?)",
        rusqlite::params!["future command", metadata],
    )
    .unwrap();
    drop(db);

    let parsed = parse_unified_arf_history(&unified_path, "r", "shell").unwrap();
    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!(parsed.entries.len(), 1);
    assert!(parsed.entries[0].metadata.is_some());

    let mut targets = create_test_targets(&target_dir);
    let result = import_entries(&mut targets, parsed.entries, None, false).unwrap();
    assert_eq!(result.r_imported, 1);
    let stored = targets
        .r_history
        .load_with_metadata(reedline::HistoryItemId::new(1))
        .unwrap();
    assert_eq!(
        serde_json::to_string(&stored.more_info.unwrap()).unwrap(),
        metadata
    );
}

#[test]
fn test_unified_export_without_metadata_column_still_imports() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let unified_path = temp_dir.path().join("old-export.db");
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    db.execute(
        r#"CREATE TABLE r (
            id INTEGER PRIMARY KEY,
            command_line TEXT NOT NULL,
            start_timestamp INTEGER
        )"#,
        [],
    )
    .unwrap();
    db.execute("INSERT INTO r (command_line) VALUES (?)", [":not metadata"])
        .unwrap();
    drop(db);

    let parsed = parse_unified_arf_history(&unified_path, "r", "shell").unwrap();
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].metadata, None);
    assert!(parsed.warnings.is_empty());
}

#[test]
fn test_parse_unified_arf_history_custom_table_names() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let unified_path = temp_dir.path().join("custom.db");

    // Create a unified export file with custom table names
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    db.execute(
            "CREATE TABLE my_r_history (id INTEGER PRIMARY KEY, command_line TEXT NOT NULL, start_timestamp INTEGER)",
            [],
        )
        .unwrap();
    db.execute(
        "INSERT INTO my_r_history (command_line) VALUES ('test_cmd')",
        [],
    )
    .unwrap();
    drop(db);

    // Parse with custom table names
    let entries =
        parse_unified_arf_history(&unified_path, "my_r_history", "my_shell_history").unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].command, "test_cmd");
    assert_eq!(entries[0].mode, Some("r".to_string()));
}

#[test]
fn test_parse_unified_arf_history_missing_tables() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let unified_path = temp_dir.path().join("empty.db");

    // Create an empty database (no tables)
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    drop(db);

    // Should return empty vec, not error
    let entries = parse_unified_arf_history(&unified_path, "r", "shell").unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_validate_table_name_valid() {
    assert!(validate_table_name("r").is_ok());
    assert!(validate_table_name("shell").is_ok());
    assert!(validate_table_name("my_r_history").is_ok());
    assert!(validate_table_name("R_History_2024").is_ok());
    assert!(validate_table_name("_private").is_ok());
}

#[test]
fn test_validate_table_name_invalid() {
    // Empty
    assert!(validate_table_name("").is_err());

    // SQL injection attempts
    assert!(validate_table_name("r; DROP TABLE history;--").is_err());
    assert!(validate_table_name("r' OR '1'='1").is_err());
    assert!(validate_table_name("table-name").is_err());
    assert!(validate_table_name("table.name").is_err());

    // Starts with digit
    assert!(validate_table_name("123table").is_err());

    // Special characters
    assert!(validate_table_name("table name").is_err());
    assert!(validate_table_name("table\nname").is_err());

    // Underscore-only names (confusing and should be rejected)
    assert!(validate_table_name("_").is_err());
    assert!(validate_table_name("___").is_err());
    assert!(validate_table_name("_____").is_err());
}

/// Test that parse_unified_arf_history works even when file is named r.db
/// This verifies that the unified parser doesn't rely on filename.
#[test]
fn test_parse_unified_works_regardless_of_filename() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    // Name the file "r.db" - traditionally a single-database file
    let unified_path = temp_dir.path().join("r.db");

    // But create it as a unified file with both r and shell tables
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    db.execute(
            "CREATE TABLE r (id INTEGER PRIMARY KEY, command_line TEXT NOT NULL, start_timestamp INTEGER)",
            [],
        )
        .unwrap();
    db.execute(
            "CREATE TABLE shell (id INTEGER PRIMARY KEY, command_line TEXT NOT NULL, start_timestamp INTEGER)",
            [],
        )
        .unwrap();
    db.execute("INSERT INTO r (command_line) VALUES ('r_cmd')", [])
        .unwrap();
    db.execute("INSERT INTO shell (command_line) VALUES ('shell_cmd')", [])
        .unwrap();
    drop(db);

    // parse_unified_arf_history should work regardless of filename
    let entries = parse_unified_arf_history(&unified_path, "r", "shell").unwrap();

    assert_eq!(entries.len(), 2);
    let r_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.mode.as_deref() == Some("r"))
        .collect();
    let shell_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.mode.as_deref() == Some("shell"))
        .collect();
    assert_eq!(r_entries.len(), 1);
    assert_eq!(r_entries[0].command, "r_cmd");
    assert_eq!(shell_entries.len(), 1);
    assert_eq!(shell_entries[0].command, "shell_cmd");
}

#[test]
fn test_parse_unified_rejects_same_table_names() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let unified_path = temp_dir.path().join("backup.db");

    // Create a minimal database
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    db.execute(
        "CREATE TABLE history (id INTEGER PRIMARY KEY, command_line TEXT)",
        [],
    )
    .unwrap();
    drop(db);

    // Parsing with same table names should fail
    let result = parse_unified_arf_history(&unified_path, "history", "history");

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("must be different"));
}

#[test]
fn test_parse_unified_with_sqlite_reserved_words_as_table_names() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let unified_path = temp_dir.path().join("export.db");

    // Create a database with SQLite reserved words as table names
    let db = rusqlite::Connection::open(&unified_path).unwrap();
    db.execute(
            r#"CREATE TABLE "select" (id INTEGER PRIMARY KEY, command_line TEXT NOT NULL, start_timestamp INTEGER)"#,
            [],
        )
        .unwrap();
    db.execute(
            r#"CREATE TABLE "from" (id INTEGER PRIMARY KEY, command_line TEXT NOT NULL, start_timestamp INTEGER)"#,
            [],
        )
        .unwrap();
    db.execute(
        r#"INSERT INTO "select" (command_line) VALUES ('r_cmd')"#,
        [],
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO "from" (command_line) VALUES ('shell_cmd')"#,
        [],
    )
    .unwrap();
    drop(db);

    // SQLite reserved words should work when quoted
    let entries = parse_unified_arf_history(&unified_path, "select", "from").unwrap();

    assert_eq!(entries.len(), 2);
    let r_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.mode.as_deref() == Some("r"))
        .collect();
    let shell_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.mode.as_deref() == Some("shell"))
        .collect();
    assert_eq!(r_entries.len(), 1);
    assert_eq!(r_entries[0].command, "r_cmd");
    assert_eq!(shell_entries.len(), 1);
    assert_eq!(shell_entries[0].command, "shell_cmd");
}
