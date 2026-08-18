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
fn metadata_backfills_only_a_metadata_less_duplicate() {
    let mut fixture = ImportFixture::new();
    fixture.import([r("backfill")]);
    let result = fixture.import_with(
        [r("backfill").with_metadata(r#"{"future":true}"#)],
        ImportOptions {
            hostname_override: None,
            skip_duplicates: true,
        },
    );
    assert_eq!(result.metadata_backfilled, 1);
    assert_eq!(result.duplicates_skipped, 0);
    assert!(fixture.r_items()[0].more_info.is_some());
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
    assert_eq!(result.metadata_backfilled, 0);
    assert_eq!(result.warnings.len(), 1);
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
    assert_eq!(result.metadata_backfilled, 1);
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
