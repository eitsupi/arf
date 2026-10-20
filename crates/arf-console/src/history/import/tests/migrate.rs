use super::super::ImportEntry;
use super::super::migrate::{MigrationOutcome, MigrationReport, migrate_history, report_outcome};
use super::support::*;
use crate::history::artifact::{HistoryKind, legacy_migration_recorded};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Dirs {
    _root: tempfile::TempDir,
    legacy: PathBuf,
    current: PathBuf,
}

impl Dirs {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        let current = root.path().join("current");
        std::fs::create_dir_all(&legacy).unwrap();
        Self {
            _root: root,
            legacy,
            current,
        }
    }

    fn run(&self, dry_run: bool) -> anyhow::Result<MigrationOutcome> {
        migrate_history(&self.legacy, &self.current, dry_run)
    }

    fn migrate(&self) -> MigrationReport {
        report(self.run(false).unwrap())
    }
}

fn report(outcome: MigrationOutcome) -> MigrationReport {
    match outcome {
        MigrationOutcome::Migrated(report) => *report,
        other => panic!("expected a migration, got {other:?}"),
    }
}

/// The error `report_outcome` raises for a report, which must be an incomplete one.
fn incomplete_error(report: MigrationReport) -> anyhow::Error {
    report_outcome(&MigrationOutcome::Migrated(Box::new(report))).unwrap_err()
}

fn rich(command: &str, ts: &str) -> ImportEntry {
    r(command)
        .at(timestamp(ts))
        .with_metadata(r#"{"future":true}"#)
        .with_standard_fields()
}

/// Bytes of every `.db` file under `dir`, to prove databases were left untouched.
///
/// Read-only connections to a WAL-mode database may leave empty `-wal`/`-shm`
/// sidecars behind, so only the main database files are compared.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "db"))
                .map(|path| {
                    (
                        path.file_name().unwrap().to_string_lossy().into_owned(),
                        std::fs::read(&path).unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn same_directory_is_a_noop_even_through_a_different_spelling() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("a", "2024-01-01T00:00:00Z").item],
    );
    let before = snapshot(&dirs.legacy);

    let outcome = migrate_history(&dirs.legacy, &dirs.legacy, false).unwrap();
    assert!(matches!(outcome, MigrationOutcome::SameLocation(_)));
    let respelled = dirs.legacy.join(".");
    let outcome = migrate_history(&dirs.legacy, &respelled, false).unwrap();
    assert!(matches!(outcome, MigrationOutcome::SameLocation(_)));
    assert_eq!(snapshot(&dirs.legacy), before);
}

#[test]
fn empty_or_missing_legacy_creates_nothing() {
    let dirs = Dirs::new();
    assert!(matches!(
        dirs.run(false).unwrap(),
        MigrationOutcome::NoLegacyHistory(_)
    ));
    std::fs::remove_dir(&dirs.legacy).unwrap();
    assert!(matches!(
        dirs.run(false).unwrap(),
        MigrationOutcome::NoLegacyHistory(_)
    ));
    assert!(!dirs.current.exists());
}

#[test]
fn r_only_legacy_populates_only_r_history() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("1 + 1", "2024-01-01T00:00:00Z").item],
    );

    let report = dirs.migrate();
    assert!(report.legacy_has_r && !report.legacy_has_shell);
    assert_eq!(report.result.r_imported, 1);
    assert_eq!(report.result.shell_imported, 0);
    assert_eq!(
        open_items(&dirs.current.join("r.db"), HistoryKind::R).len(),
        1
    );
    assert!(open_items(&dirs.current.join("shell.db"), HistoryKind::Shell).is_empty());
}

#[test]
fn shell_only_legacy_populates_only_shell_history() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("shell.db"),
        &[shell("ls").at(timestamp("2024-01-01T00:00:00Z")).item],
    );

    let report = dirs.migrate();
    assert!(!report.legacy_has_r && report.legacy_has_shell);
    assert_eq!(report.result.shell_imported, 1);
    assert_eq!(report.result.r_imported, 0);
    assert!(open_items(&dirs.current.join("r.db"), HistoryKind::R).is_empty());
    assert_eq!(
        open_items(&dirs.current.join("shell.db"), HistoryKind::Shell)[0].command_line,
        "ls"
    );
}

#[test]
fn both_databases_keep_their_metadata_and_kind() {
    let dirs = Dirs::new();
    let r_item = rich("model |> summary()", "2024-06-15T14:30:45Z").item;
    let shell_item = shell("git status")
        .at(timestamp("2024-06-15T14:31:00Z"))
        .with_standard_fields()
        .item;
    create_reedline_db(&dirs.legacy.join("r.db"), std::slice::from_ref(&r_item));
    create_reedline_db(
        &dirs.legacy.join("shell.db"),
        std::slice::from_ref(&shell_item),
    );

    let report = dirs.migrate();
    assert_eq!(report.result.r_imported, 1);
    assert_eq!(report.result.shell_imported, 1);
    assert_eq!(
        idless(open_items(&dirs.current.join("r.db"), HistoryKind::R).remove(0)),
        idless(r_item)
    );
    assert_eq!(
        idless(open_items(&dirs.current.join("shell.db"), HistoryKind::Shell).remove(0)),
        idless(shell_item)
    );
}

#[test]
fn existing_destination_is_deduplicated_and_rerun_is_idempotent() {
    let dirs = Dirs::new();
    let shared = rich("shared", "2024-01-01T00:00:00Z").item;
    let only_legacy = rich("only legacy", "2024-01-02T00:00:00Z").item;
    create_reedline_db(&dirs.legacy.join("r.db"), &[shared.clone(), only_legacy]);
    std::fs::create_dir_all(&dirs.current).unwrap();
    create_reedline_db(
        &dirs.current.join("r.db"),
        &[shared, rich("only current", "2024-01-03T00:00:00Z").item],
    );

    let first = dirs.migrate();
    assert!(first.current_had_r && !first.current_had_shell);
    assert_eq!(first.result.r_imported, 1);
    assert_eq!(first.result.duplicates_skipped, 1);
    assert_eq!(
        open_items(&dirs.current.join("r.db"), HistoryKind::R).len(),
        3
    );

    let second = dirs.migrate();
    assert_eq!(second.result.r_imported, 0);
    assert_eq!(second.result.duplicates_skipped, 2);
    assert_eq!(
        open_items(&dirs.current.join("r.db"), HistoryKind::R).len(),
        3
    );
}

#[test]
fn destination_rows_missing_fields_are_repaired_from_legacy() {
    let dirs = Dirs::new();
    let full = rich("repair me", "2024-01-01T00:00:00Z").item;
    let mut sparse = full.clone();
    sparse.cwd = None;
    sparse.exit_status = None;
    create_reedline_db(&dirs.legacy.join("r.db"), std::slice::from_ref(&full));
    std::fs::create_dir_all(&dirs.current).unwrap();
    create_reedline_db(&dirs.current.join("r.db"), &[sparse]);

    let report = dirs.migrate();
    assert_eq!(report.result.duplicates_repaired, 1);
    let migrated = open_items(&dirs.current.join("r.db"), HistoryKind::R).remove(0);
    assert_eq!(migrated.cwd, full.cwd);
    assert_eq!(migrated.exit_status, full.exit_status);
}

#[test]
fn migration_never_modifies_the_legacy_directory() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("a", "2024-01-01T00:00:00Z").item],
    );
    create_reedline_db(&dirs.legacy.join("shell.db"), &[shell("ls").item]);
    let before = snapshot(&dirs.legacy);

    dirs.migrate();
    dirs.migrate();
    dirs.run(true).unwrap();
    assert_eq!(snapshot(&dirs.legacy), before);
}

#[test]
fn dry_run_creates_nothing_and_reports_against_a_missing_destination() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("a", "2024-01-01T00:00:00Z").item],
    );

    let report = report(dirs.run(true).unwrap());
    assert!(report.dry_run);
    assert_eq!(report.result.r_imported, 1);
    assert!(!dirs.current.exists());
}

#[test]
fn dry_run_uses_existing_destination_for_dedup_without_changing_it() {
    let dirs = Dirs::new();
    let shared = rich("shared", "2024-01-01T00:00:00Z").item;
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[shared.clone(), rich("new", "2024-01-02T00:00:00Z").item],
    );
    std::fs::create_dir_all(&dirs.current).unwrap();
    create_reedline_db(&dirs.current.join("r.db"), &[shared]);
    let before = snapshot(&dirs.current);

    let report = report(dirs.run(true).unwrap());
    assert_eq!(report.result.r_imported, 1);
    assert_eq!(report.result.duplicates_skipped, 1);
    assert_eq!(snapshot(&dirs.current), before);
}

#[test]
fn unreadable_legacy_source_names_its_path_and_leaves_destination_untouched() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("a", "2024-01-01T00:00:00Z").item],
    );
    let broken = dirs.legacy.join("shell.db");
    std::fs::write(&broken, b"this is not a sqlite database, just text padding").unwrap();

    let error = format!("{:#}", dirs.run(false).unwrap_err());
    assert!(error.contains(&broken.display().to_string()), "{error}");
    assert!(error.contains("Nothing was written"), "{error}");
    assert!(!dirs.current.exists());
}

#[test]
fn legacy_database_of_the_wrong_kind_is_rejected() {
    let dirs = Dirs::new();
    let store = crate::history::HistoryStore::open(
        dirs.legacy.join("r.db"),
        HistoryKind::Shell,
        None,
        None,
    )
    .unwrap();
    drop(store);

    let error = format!("{:#}", dirs.run(false).unwrap_err());
    assert!(error.contains("expected a 'r' history database"), "{error}");
    assert!(!dirs.current.exists());
}

#[test]
fn destination_failure_names_the_path_and_a_rerun_completes_the_migration() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("a", "2024-01-01T00:00:00Z").item],
    );
    create_reedline_db(&dirs.legacy.join("shell.db"), &[shell("ls").item]);
    let blocked = dirs.current.join("shell.db");
    std::fs::create_dir_all(&blocked).unwrap();
    let legacy_before = snapshot(&dirs.legacy);

    let error = format!("{:#}", dirs.run(false).unwrap_err());
    assert!(error.contains(&blocked.display().to_string()), "{error}");
    assert!(error.contains("safe"), "{error}");
    assert_eq!(snapshot(&dirs.legacy), legacy_before);

    std::fs::remove_dir(&blocked).unwrap();
    let report = dirs.migrate();
    assert_eq!(
        report.result.r_imported + report.result.duplicates_skipped,
        1
    );
    assert_eq!(report.result.shell_imported, 1);
    assert_eq!(
        open_items(&dirs.current.join("r.db"), HistoryKind::R).len(),
        1
    );
}

#[cfg(unix)]
#[test]
fn unreadable_legacy_directory_is_an_error_not_an_empty_legacy() {
    use std::os::unix::fs::PermissionsExt;

    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("a", "2024-01-01T00:00:00Z").item],
    );
    std::fs::set_permissions(&dirs.legacy, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Privileged users can still inspect the directory, so the failure cannot be provoked.
    let provoked = dirs.legacy.join("r.db").try_exists().is_err();

    let outcome = dirs.run(false);
    std::fs::set_permissions(&dirs.legacy, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !provoked {
        return;
    }

    let error = format!("{:#}", outcome.unwrap_err());
    assert!(error.contains("Failed to inspect"), "{error}");
    assert!(
        error.contains(&dirs.legacy.join("r.db").display().to_string()),
        "{error}"
    );
    assert!(!dirs.current.exists());
}

/// Make the destination reject writes touching `command`, as a failing disk would.
fn block_writes_for(db: &Path, event: &str, command: &str) {
    let connection = rusqlite::Connection::open(db).unwrap();
    connection
        .execute_batch(&format!(
            r#"CREATE TRIGGER block_{event} BEFORE {event} ON history
               WHEN NEW.command_line = '{command}'
               BEGIN SELECT RAISE(ABORT, 'blocked by test'); END"#
        ))
        .unwrap();
}

#[test]
fn rows_that_cannot_be_written_fail_the_migration_but_the_rest_are_copied() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[
            rich("blocked", "2024-01-01T00:00:00Z").item,
            rich("fine", "2024-01-02T00:00:00Z").item,
        ],
    );
    std::fs::create_dir_all(&dirs.current).unwrap();
    create_reedline_db(&dirs.current.join("r.db"), &[]);
    block_writes_for(&dirs.current.join("r.db"), "INSERT", "blocked");

    let report = dirs.migrate();
    assert_eq!(report.result.write_failures, 1);
    assert_eq!(report.result.r_imported, 1);
    let error = format!("{:#}", incomplete_error(report));
    assert!(
        error.contains(&dirs.current.display().to_string()),
        "{error}"
    );
    assert!(error.contains("not modified"), "{error}");
    assert!(error.contains("safe"), "{error}");
    let copied = open_items(&dirs.current.join("r.db"), HistoryKind::R);
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].command_line, "fine");
}

#[test]
fn rows_that_cannot_be_repaired_fail_the_migration() {
    let dirs = Dirs::new();
    let full = rich("repair me", "2024-01-01T00:00:00Z").item;
    let mut sparse = full.clone();
    sparse.cwd = None;
    create_reedline_db(&dirs.legacy.join("r.db"), std::slice::from_ref(&full));
    std::fs::create_dir_all(&dirs.current).unwrap();
    create_reedline_db(&dirs.current.join("r.db"), &[sparse]);
    block_writes_for(&dirs.current.join("r.db"), "UPDATE", "repair me");

    let report = dirs.migrate();
    assert_eq!(report.result.write_failures, 1);
    incomplete_error(report);
}

#[test]
fn legacy_parse_warnings_are_reported_for_real_and_dry_runs() {
    let dirs = Dirs::new();
    create_reedline_db(&dirs.legacy.join("r.db"), &[r("bad metadata").item]);
    let db = rusqlite::Connection::open(dirs.legacy.join("r.db")).unwrap();
    db.execute(
        r#"UPDATE history SET more_info = ? WHERE command_line = ?"#,
        rusqlite::params![r#"{"unterminated"#, "bad metadata"],
    )
    .unwrap();
    drop(db);
    create_reedline_db(&dirs.legacy.join("shell.db"), &[shell("also bad").item]);
    let db = rusqlite::Connection::open(dirs.legacy.join("shell.db")).unwrap();
    db.execute(r#"UPDATE history SET more_info = ?"#, [r#"{"unterminated"#])
        .unwrap();
    drop(db);

    for dry_run in [true, false] {
        let report = report(dirs.run(dry_run).unwrap());
        assert_eq!(report.result.warnings.len(), 2, "dry_run={dry_run}");
    }
}

fn recorded(dirs: &Dirs) -> [bool; 2] {
    [
        legacy_migration_recorded(&dirs.current.join("r.db")).unwrap(),
        legacy_migration_recorded(&dirs.current.join("shell.db")).unwrap(),
    ]
}

#[test]
fn completed_migration_records_itself_in_both_current_databases_only() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("1 + 1", "2024-01-01T00:00:00Z").item],
    );

    dirs.migrate();
    // Even the kind with no legacy database is marked, so one record covers
    // the whole current directory.
    assert_eq!(recorded(&dirs), [true, true]);
    // The marker is optional metadata: the databases still open normally.
    assert_eq!(
        open_items(&dirs.current.join("r.db"), HistoryKind::R).len(),
        1
    );
    // Rerunning keeps the single record.
    dirs.migrate();
    assert_eq!(recorded(&dirs), [true, true]);
}

#[test]
fn opening_a_database_never_records_a_migration() {
    let dirs = Dirs::new();
    std::fs::create_dir_all(&dirs.current).unwrap();
    open_items(&dirs.current.join("r.db"), HistoryKind::R);
    assert!(!legacy_migration_recorded(&dirs.current.join("r.db")).unwrap());
}

#[test]
fn dry_run_and_incomplete_migrations_record_nothing() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("1 + 1", "2024-01-01T00:00:00Z").item],
    );
    report(dirs.run(true).unwrap());
    assert!(!dirs.current.exists());

    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("blocked", "2024-01-01T00:00:00Z").item],
    );
    std::fs::create_dir_all(&dirs.current).unwrap();
    create_reedline_db(&dirs.current.join("r.db"), &[]);
    block_writes_for(&dirs.current.join("r.db"), "INSERT", "blocked");
    let report = dirs.migrate();
    assert_eq!(report.result.write_failures, 1);
    assert_eq!(recorded(&dirs), [false, false]);
}

#[test]
fn failing_to_record_the_migration_makes_it_incomplete() {
    let dirs = Dirs::new();
    create_reedline_db(
        &dirs.legacy.join("r.db"),
        &[rich("1 + 1", "2024-01-01T00:00:00Z").item],
    );
    // A destination without arf metadata cannot carry the record.
    std::fs::create_dir_all(&dirs.current).unwrap();
    create_reedline_db(&dirs.current.join("r.db"), &[]);

    let report = dirs.migrate();
    assert_eq!(report.result.write_failures, 0);
    assert_eq!(report.record_failures, 1);
    assert!(
        report
            .result
            .warnings
            .iter()
            .any(|warning| warning.contains("Could not record the completed migration"))
    );
    assert_eq!(recorded(&dirs), [false, true]);
    let error = format!("{:#}", incomplete_error(report));
    assert!(error.contains("incomplete"), "{error}");
    assert!(error.contains("could not be recorded"), "{error}");
    assert!(error.contains("safe"), "{error}");
}
