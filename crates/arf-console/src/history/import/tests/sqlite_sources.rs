use super::super::*;
use super::support::*;
use std::time::Duration;

#[test]
fn arf_source_infers_filename_mode_and_round_trips_a_rich_item() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("r.db");
    let source_item = r("rich arf")
        .at(timestamp("2024-06-15T14:30:45Z"))
        .with_metadata(r#"{"future":true}"#)
        .with_standard_fields()
        .item;
    create_reedline_db(&source, std::slice::from_ref(&source_item));

    let parsed = parse_arf_history(&source).unwrap();
    assert_eq!(parsed.entries[0].mode, ImportMode::R);
    assert_eq!(
        idless(parsed.entries[0].item.clone()),
        idless(source_item.clone())
    );

    let shell_source = dir.path().join("shell.db");
    std::fs::copy(&source, &shell_source).unwrap();
    assert_eq!(
        parse_arf_history(&shell_source).unwrap().entries[0].mode,
        ImportMode::Shell
    );

    let mut fixture = ImportFixture::new();
    fixture.import(parsed.entries);
    assert_eq!(idless(fixture.r_items().remove(0)), idless(source_item));
}

#[test]
fn unified_source_round_trips_r_and_shell_rich_rows() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("export.db");
    let db = rusqlite::Connection::open(&source).unwrap();
    create_export_table(
        &db,
        "r",
        ExportColumns::Full,
        &[DbRow {
            command: "r rich",
            timestamp_ms: Some(timestamp("2024-06-15T14:30:45Z").timestamp_millis()),
            session_id: Some(987654321),
            hostname: Some("export-host"),
            cwd: Some("/export/cwd"),
            duration_ms: Some(4321),
            exit_status: Some(23),
            metadata: Some(r#"{"future":{"value":42}}"#),
        }],
    );
    create_export_table(
        &db,
        "shell",
        ExportColumns::Full,
        &[DbRow {
            command: "shell rich",
            metadata: Some(r#"{"shell":true}"#),
            ..DbRow::default()
        }],
    );
    drop(db);

    let parsed = parse_unified_arf_history(&source, "r", "shell").unwrap();
    assert_eq!(parsed.entries.len(), 2);
    assert_eq!(parsed.entries[0].mode, ImportMode::R);
    assert_eq!(parsed.entries[1].mode, ImportMode::Shell);
    assert_eq!(
        parsed.entries[0].item.duration,
        Some(Duration::from_millis(4321))
    );
    assert_eq!(parsed.entries[0].item.exit_status, Some(23));
    assert!(parsed.entries[0].item.more_info.is_some());

    let expected_r = idless(parsed.entries[0].item.clone());
    let expected_shell = idless(parsed.entries[1].item.clone());
    let mut fixture = ImportFixture::new();
    fixture.import(parsed.entries);
    assert_eq!(idless(fixture.r_items().remove(0)), expected_r);
    assert_eq!(idless(fixture.shell_items().remove(0)), expected_shell);
}

#[test]
fn legacy_tables_and_missing_tables_remain_compatible() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("legacy.db");
    let db = rusqlite::Connection::open(&legacy).unwrap();
    create_export_table(
        &db,
        "r",
        ExportColumns::LegacyMinimal,
        &[DbRow {
            command: "legacy",
            ..DbRow::default()
        }],
    );
    drop(db);
    let parsed = parse_unified_arf_history(&legacy, "r", "shell").unwrap();
    assert_eq!(parsed.entries.len(), 1);
    assert!(parsed.entries[0].item.more_info.is_none());
    assert!(parsed.entries[0].item.hostname.is_none());

    let empty = dir.path().join("empty.db");
    rusqlite::Connection::open(&empty).unwrap();
    let error = parse_unified_arf_history(&empty, "r", "shell").unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            r#"File '{}' does not look like an arf export: missing configured history tables 'r' and 'shell'"#,
            empty.display()
        )
    );
}

#[test]
fn unified_source_rejects_database_with_only_unconfigured_tables() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("unrelated.db");
    let db = rusqlite::Connection::open(&source).unwrap();
    create_export_table(
        &db,
        "other",
        ExportColumns::LegacyMinimal,
        &[DbRow {
            command: "unrelated",
            ..DbRow::default()
        }],
    );
    drop(db);

    let error = parse_unified_arf_history(&source, "custom_r", "custom_shell").unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            r#"File '{}' does not look like an arf export: missing configured history tables 'custom_r' and 'custom_shell'"#,
            source.display()
        )
    );
}

#[test]
fn malformed_metadata_degrades_per_row_and_imports_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("r.db");
    let valid = r("valid").with_metadata(r#"{"healthy":true}"#).item;
    let rows = vec![r("malformed").item, valid, r("null").item];
    create_reedline_db(&source, &rows);
    let db = rusqlite::Connection::open(&source).unwrap();
    db.execute(
        r#"UPDATE history SET more_info = ? WHERE command_line = ?"#,
        rusqlite::params![r#"{"unterminated"#, "malformed"],
    )
    .unwrap();
    drop(db);

    let parsed = parse_arf_history(&source).unwrap();
    assert_eq!(parsed.entries.len(), 3);
    assert_eq!(parsed.entries[0].item.more_info, None);
    assert!(parsed.entries[1].item.more_info.is_some());
    assert_eq!(parsed.entries[2].item.more_info, None);
    assert_eq!(parsed.warnings.len(), 1);

    let mut fixture = ImportFixture::new();
    let result = fixture.import(parsed.entries);
    assert_eq!(result.r_imported, 3);
    assert!(fixture.r_items()[1].more_info.is_some());
    assert_eq!(fixture.r_items()[0].more_info, None);
    assert_eq!(fixture.r_items()[2].more_info, None);
}

#[test]
fn negative_duration_is_warned_and_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("negative.db");
    let db = rusqlite::Connection::open(&source).unwrap();
    create_export_table(
        &db,
        "r",
        ExportColumns::Full,
        &[DbRow {
            command: "negative duration",
            duration_ms: Some(-1),
            ..DbRow::default()
        }],
    );
    drop(db);

    let parsed = parse_unified_arf_history(&source, "r", "shell").unwrap();
    assert_eq!(parsed.entries[0].item.duration, None);
    assert_eq!(parsed.warnings.len(), 1);
    assert!(parsed.warnings[0].contains("negative duration"));
    let mut fixture = ImportFixture::new();
    fixture.import(parsed.entries);
    assert_eq!(fixture.r_items()[0].duration, None);
}

#[test]
fn source_boundaries_and_table_name_validation_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    let db = rusqlite::Connection::open(&source).unwrap();
    create_export_table(
        &db,
        "select",
        ExportColumns::LegacyMinimal,
        &[DbRow {
            command: "reserved",
            ..DbRow::default()
        }],
    );
    create_export_table(
        &db,
        "from",
        ExportColumns::LegacyMinimal,
        &[DbRow {
            command: "shell reserved",
            ..DbRow::default()
        }],
    );
    drop(db);
    let parsed = parse_unified_arf_history(&source, "select", "from").unwrap();
    assert_eq!(parsed.entries.len(), 2);

    assert!(validate_table_name("my_r_history").is_ok());
    for invalid in [
        "",
        r#"r; DROP TABLE history;--"#,
        r#"r' OR '1'='1"#,
        "123table",
        "_",
    ] {
        assert!(validate_table_name(invalid).is_err(), "{invalid}");
    }
    let same = parse_unified_arf_history(&source, "select", "select").unwrap_err();
    assert!(same.to_string().contains("must be different"));
}

#[test]
fn rejects_missing_and_non_history_arf_databases() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.db");
    assert!(parse_arf_history(&missing).is_err());
    let not_history = dir.path().join("not-history.db");
    rusqlite::Connection::open(&not_history).unwrap();
    assert!(parse_arf_history(&not_history).is_err());
}
