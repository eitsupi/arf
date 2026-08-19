use super::super::*;
use super::support::*;

#[test]
fn dedup_key_matrix_is_command_and_optional_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let r_path = dir.path().join("r.db");
    let shell_path = dir.path().join("shell.db");
    let ts = timestamp("2024-06-15T14:30:45Z");
    create_reedline_db(&r_path, &[r("same").at(ts).item, r("untimestamped").item]);
    create_reedline_db(&shell_path, &[shell("same").item]);
    let r_dedup = DedupSet::from_db(&r_path).unwrap();
    let shell_dedup = DedupSet::from_db(&shell_path).unwrap();

    let cases = [
        (r("same").at(ts), EntryPlan::Duplicate),
        (
            r("same").at(timestamp("2024-06-15T14:30:46Z")),
            EntryPlan::Insert(ImportTarget::R),
        ),
        (r("untimestamped"), EntryPlan::Duplicate),
        (r("different"), EntryPlan::Insert(ImportTarget::R)),
        (shell("same"), EntryPlan::Duplicate),
    ];
    for (input, expected) in cases {
        let plan = plan_entry(&input, Some(&r_dedup), Some(&shell_dedup));
        assert_eq!(plan, expected);
    }
}

#[test]
fn idempotent_import_and_duplicate_disabled_import_are_distinct() {
    let mut fixture = ImportFixture::new();
    fixture.import([r("same")]);
    let duplicate = fixture.import_with(
        [r("same")],
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );
    assert_eq!(duplicate.duplicates_skipped, 1);

    let mut disabled = ImportFixture::new();
    disabled.import([r("same")]);
    disabled.import([r("same")]);
    assert_eq!(disabled.r_items().len(), 2);
}

#[test]
fn duplicate_repair_fills_missing_fields_even_without_metadata() {
    let mut fixture = ImportFixture::new();
    fixture.import([r("backfill")]);
    let result = fixture.import_with(
        [r("backfill").with_metadata(r#"{"future":true}"#)],
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );
    assert_eq!(result.duplicates_repaired, 1);
    assert_eq!(result.duplicates_skipped, 0);
    assert!(fixture.r_items()[0].more_info.is_some());
}

#[test]
fn duplicate_repair_fills_all_missing_fields_from_an_old_source() {
    let timestamp = timestamp("2024-06-15T14:30:45Z");
    let source = r("old importer").at(timestamp).with_standard_fields().item;
    let mut fixture = ImportFixture::new();
    fixture.import([r("old importer").at(timestamp)]);

    let result = fixture.import_with(
        [ImportEntry {
            mode: ImportMode::R,
            item: source.clone(),
        }],
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );

    assert_eq!(result.duplicates_repaired, 1);
    assert_eq!(result.r_imported, 0);
    assert_eq!(fixture.r_items()[0].session_id, source.session_id);
    assert_eq!(fixture.r_items()[0].hostname, source.hostname);
    assert_eq!(fixture.r_items()[0].cwd, source.cwd);
    assert_eq!(fixture.r_items()[0].duration, source.duration);
    assert_eq!(fixture.r_items()[0].exit_status, source.exit_status);
    assert_eq!(fixture.r_items()[0].more_info, None);
}

#[test]
fn duplicate_repair_preserves_hostname_set_by_hostname_override() {
    let timestamp = timestamp("2024-06-15T14:30:45Z");
    let source = r("preserve hostname")
        .at(timestamp)
        .with_standard_fields()
        .item;
    let mut fixture = ImportFixture::new();
    fixture.import_with(
        [r("preserve hostname").at(timestamp)],
        ImportOptions {
            hostname_override: Some("chosen-host"),
            skip_duplicates: false,
        },
    );

    let result = fixture.import_with(
        [ImportEntry {
            mode: ImportMode::R,
            item: source,
        }],
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );

    let item = &fixture.r_items()[0];
    assert_eq!(result.duplicates_repaired, 1);
    assert_eq!(item.hostname.as_deref(), Some("chosen-host"));
    assert_eq!(item.cwd.as_deref(), Some("/fixture/cwd"));
    assert_eq!(item.exit_status, Some(17));
}

#[test]
fn stale_duplicate_repair_skips_when_command_line_changed() {
    let mut fixture = ImportFixture::new();
    fixture.import([r("selected")]);
    let path = fixture.targets.r_history.path().unwrap().to_owned();
    let r_dedup = DedupSet::from_db(&path).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            r#"UPDATE history SET command_line = ? WHERE id = ?"#,
            rusqlite::params!["changed", 1_i64],
        )
        .unwrap();

    let result = import_entries_with_dedup_sets(
        &mut fixture.targets,
        vec![r("selected").with_metadata(r#"{"new":true}"#)],
        None,
        Some(r_dedup),
        None,
    )
    .unwrap();

    assert_eq!(result.duplicates_skipped, 1);
    assert_eq!(result.duplicates_repaired, 0);
    assert_eq!(fixture.r_items()[0].command_line, "changed");
    assert_eq!(fixture.r_items()[0].more_info, None);
}

#[test]
fn stale_duplicate_repair_skips_when_timestamp_changed() {
    let original_timestamp = timestamp("2024-06-15T14:30:45Z");
    let changed_timestamp = timestamp("2024-06-15T14:30:46Z");
    let mut fixture = ImportFixture::new();
    fixture.import([r("timestamp selected").at(original_timestamp)]);
    let path = fixture.targets.r_history.path().unwrap().to_owned();
    let r_dedup = DedupSet::from_db(&path).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            r#"UPDATE history SET start_timestamp = ? WHERE id = ?"#,
            rusqlite::params![changed_timestamp.timestamp_millis(), 1_i64],
        )
        .unwrap();

    let result = import_entries_with_dedup_sets(
        &mut fixture.targets,
        vec![
            r("timestamp selected")
                .at(original_timestamp)
                .with_metadata(r#"{"new":true}"#),
        ],
        None,
        Some(r_dedup),
        None,
    )
    .unwrap();

    assert_eq!(result.duplicates_skipped, 1);
    assert_eq!(result.duplicates_repaired, 0);
    assert_eq!(
        fixture.r_items()[0].start_timestamp,
        Some(changed_timestamp)
    );
    assert_eq!(fixture.r_items()[0].more_info, None);
}

#[test]
fn malformed_metadata_does_not_block_other_repairs_or_change_metadata() {
    let (dir, mut targets) = malformed_target();
    import_entries(&mut targets, vec![r("broken fields")], None, false).unwrap();
    let path = dir.path().join("r.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        r#"UPDATE history SET more_info = ? WHERE command_line = ?"#,
        rusqlite::params![r#"{"broken"#, "broken fields"],
    )
    .unwrap();
    drop(db);

    let source = r("broken fields")
        .with_standard_fields()
        .with_metadata(r#"{"new":true}"#)
        .item;
    let expected_session_id = source.session_id;
    let result = import_entries(
        &mut targets,
        vec![ImportEntry {
            mode: ImportMode::R,
            item: source,
        }],
        None,
        true,
    )
    .unwrap();

    assert_eq!(result.duplicates_repaired, 1);
    assert_eq!(result.duplicates_skipped, 0);
    let db = rusqlite::Connection::open(&path).unwrap();
    let row = db
        .query_row(
            r#"SELECT session_id, hostname, cwd, duration_ms, exit_status, more_info
               FROM history WHERE command_line = ?"#,
            ["broken fields"],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, expected_session_id.map(Into::into));
    assert_eq!(row.1.as_deref(), Some("fixture-host"));
    assert_eq!(row.2.as_deref(), Some("/fixture/cwd"));
    assert_eq!(row.3, Some(1234));
    assert_eq!(row.4, Some(17));
    assert_eq!(row.5.as_deref(), Some(r#"{"broken"#));
}

#[test]
fn existing_metadata_wins_and_ambiguous_rows_warn() {
    let mut fixture = ImportFixture::new();
    fixture.import([r("known").with_metadata(r#"{"original":true}"#)]);
    let result = fixture.import_with(
        [r("known").with_metadata(r#"{"replacement":true}"#)],
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );
    assert_eq!(result.duplicates_skipped, 1);
    assert_eq!(
        fixture.r_items()[0].more_info,
        serde_json::from_str(r#"{"original":true}"#).ok()
    );

    let mut ambiguous = ImportFixture::new();
    ambiguous.import([r("ambiguous"), r("ambiguous")]);
    let result = ambiguous.import_with(
        [r("ambiguous").with_metadata(r#"{"future":true}"#)],
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );
    assert_eq!(result.duplicates_skipped, 1);
    assert_eq!(result.duplicates_repaired, 0);
    assert_eq!(result.warnings.len(), 1);
    assert!(
        ambiguous
            .r_items()
            .iter()
            .all(|item| item.more_info.is_none())
    );
}

fn malformed_target() -> (tempfile::TempDir, ImportTargets) {
    let dir = tempfile::TempDir::new().unwrap();
    let targets = ImportTargets {
        r_history: HistoryStore::open(dir.path().join("r.db"), None, None).unwrap(),
        shell_history: HistoryStore::open(dir.path().join("shell.db"), None, None).unwrap(),
    };
    (dir, targets)
}

#[test]
fn malformed_existing_metadata_is_duplicate_in_real_and_dry_paths() {
    let (dir, mut targets) = malformed_target();
    import_entries(&mut targets, vec![r("broken")], None, false).unwrap();
    let path = dir.path().join("r.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        r#"UPDATE history SET more_info = ? WHERE command_line = ?"#,
        rusqlite::params![r#"{"broken"#, "broken"],
    )
    .unwrap();
    drop(db);

    let incoming = r("broken").with_metadata(r#"{"new":true}"#);
    let real = import_entries(&mut targets, vec![incoming.clone()], None, true).unwrap();
    assert_eq!(real.duplicates_skipped, 1);
    assert_eq!(real.warnings.len(), 1);

    let dedup = DedupSet::from_db(&path).unwrap();
    let dry = import_entries_dry_run(&[incoming], Some(&dedup), None);
    assert_eq!(dry.duplicates_skipped, 1);
    assert_eq!(dry.r_imported, 0);
    assert_eq!(dry.warnings.len(), 1);
}

#[test]
fn repeated_repairs_have_the_same_dry_run_and_real_counts() {
    let timestamp = timestamp("2024-06-15T14:30:45Z");
    let source = r("repeated repair").at(timestamp).with_standard_fields();
    let entries = vec![source.clone(), source];
    let mut fixture = ImportFixture::new();
    fixture.import([r("repeated repair").at(timestamp)]);

    let dedup = DedupSet::from_db(fixture.targets.r_history.path().unwrap()).unwrap();
    let dry = import_entries_dry_run(&entries, Some(&dedup), None);
    let real = fixture.import_with(
        entries,
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );

    assert_eq!(dry, real);
    assert_eq!(dry.duplicates_repaired, 1);
    assert_eq!(dry.duplicates_skipped, 1);
    assert!(dry.warnings.is_empty());
}

#[test]
fn dry_run_dedup_is_partial_and_does_not_write_or_create_side_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        r#"CREATE TABLE history (
            id INTEGER PRIMARY KEY,
            command_line TEXT NOT NULL,
            start_timestamp INTEGER,
            more_info TEXT
        )"#,
        [],
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO history (command_line) VALUES (?)"#,
        ["existing"],
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO history (command_line) VALUES (?)"#,
        ["backfill"],
    )
    .unwrap();
    drop(db);
    let wal = path.with_extension("db-wal");
    let shm = path.with_extension("db-shm");
    assert!(!wal.exists());
    assert!(!shm.exists());

    let dedup = DedupSet::from_db(&path);
    assert!(dedup.is_ok());
    assert!(!wal.exists());
    assert!(!shm.exists());
    let dedup = dedup.unwrap();
    let result = import_entries_dry_run(
        &[
            r("existing"),
            r("backfill").with_metadata(r#"{"new":true}"#),
            shell("new shell"),
        ],
        Some(&dedup),
        None,
    );
    assert_eq!(result.duplicates_skipped, 1);
    assert_eq!(result.duplicates_repaired, 1);
    assert_eq!(result.shell_imported, 1);
    assert!(!wal.exists());
    assert!(!shm.exists());

    let read_only =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let metadata: Option<String> = read_only
        .query_row(
            r#"SELECT more_info FROM history WHERE command_line = ?"#,
            ["existing"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(metadata.is_none());
}

#[test]
fn dry_run_repair_matches_real_import_and_keeps_source_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("export.db");
    let timestamp = timestamp("2024-06-15T14:30:45Z");
    let source_item = r("dry repair").at(timestamp).with_standard_fields().item;
    let db = rusqlite::Connection::open(&source_path).unwrap();
    create_export_table(
        &db,
        "r",
        ExportColumns::Full,
        &[DbRow {
            command: "dry repair",
            timestamp_ms: Some(timestamp.timestamp_millis()),
            session_id: source_item.session_id.map(Into::into),
            hostname: source_item.hostname.as_deref(),
            cwd: source_item.cwd.as_deref(),
            duration_ms: source_item.duration.map(|value| value.as_millis() as i64),
            exit_status: source_item.exit_status,
            metadata: None,
        }],
    );
    drop(db);
    let parsed = parse_unified_arf_history(&source_path, "r", "shell").unwrap();
    assert!(!source_path.with_extension("db-wal").exists());
    assert!(!source_path.with_extension("db-shm").exists());

    let target_dir = tempfile::tempdir().unwrap();
    let r_path = target_dir.path().join("r.db");
    let shell_path = target_dir.path().join("shell.db");
    let mut targets = ImportTargets {
        r_history: HistoryStore::open(r_path.clone(), None, None).unwrap(),
        shell_history: HistoryStore::open(shell_path, None, None).unwrap(),
    };
    targets
        .r_history
        .save_imported(r("dry repair").at(timestamp).item)
        .unwrap();

    let dedup = DedupSet::from_db(&r_path).unwrap();
    let dry = import_entries_dry_run(&parsed.entries, Some(&dedup), None);
    let real = import_entries(&mut targets, parsed.entries, None, true).unwrap();

    assert_eq!(dry, real);
    assert_eq!(dry.duplicates_repaired, 1);
}
