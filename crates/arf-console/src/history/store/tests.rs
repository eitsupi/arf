use super::*;
use serde_json::Value;

fn injected_failure(message: &str) -> Result<HistoryStore> {
    Err(reedline::ReedlineError(
        reedline::ReedlineErrorVariants::HistoryDatabaseError(message.to_string()),
    ))
}

fn open_store() -> (tempfile::TempDir, HistoryStore) {
    let dir = tempfile::tempdir().unwrap();
    let store =
        HistoryStore::open(dir.path().join("history.db"), HistoryKind::R, None, None).unwrap();
    (dir, store)
}

fn existing_database_with_metadata(
    path: &std::path::Path,
    artifact_name: &str,
    format_version: &str,
    history_kind: Option<&str>,
) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            r#"CREATE TABLE arf_metadata (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                )"#,
        )
        .unwrap();
    let mut entries = vec![
        ("artifact", artifact_name),
        ("format_version", format_version),
        ("created_by_version", env!("CARGO_PKG_VERSION")),
    ];
    if let Some(history_kind) = history_kind {
        entries.push(("history_kind", history_kind));
    }
    for (key, value) in entries {
        connection
            .execute(
                "INSERT INTO arf_metadata (key, value) VALUES (?1, ?2)",
                [key, value],
            )
            .unwrap();
    }
}

fn assert_no_history_table(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "reedline history schema must not be created");
}

#[test]
fn newly_created_persistent_stores_write_kind_metadata() {
    let dir = tempfile::tempdir().unwrap();
    for (filename, kind) in [("r.db", HistoryKind::R), ("shell.db", HistoryKind::Shell)] {
        let path = dir.path().join(filename);
        drop(HistoryStore::open(path.clone(), kind, None, None).unwrap());
        let connection = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            artifact::read_artifact(&connection).unwrap(),
            artifact::HistoryArtifact::History(kind)
        );
        let metadata: std::collections::HashMap<String, String> = connection
            .prepare("SELECT key, value FROM arf_metadata")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(metadata.len(), 4);
        assert_eq!(
            metadata.get("artifact").map(String::as_str),
            Some("history")
        );
        assert_eq!(
            metadata.get("format_version").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            metadata.get("history_kind").map(String::as_str),
            Some(kind.as_str())
        );
        assert_eq!(
            metadata.get("created_by_version").map(String::as_str),
            Some(env!("CARGO_PKG_VERSION"))
        );
        drop(connection);
        drop(HistoryStore::open(path, kind, None, None).unwrap());
    }
}

#[test]
fn published_metadata_only_database_can_be_opened_by_reedline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata-only.db");
    artifact::prepare_history_path(&path, HistoryKind::R).unwrap();

    let connection = rusqlite::Connection::open(&path).unwrap();
    let history_table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history_table_count, 0);
    drop(connection);

    drop(HistoryStore::open(path.clone(), HistoryKind::R, None, None).unwrap());

    let connection = rusqlite::Connection::open(path).unwrap();
    let history_table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history_table_count, 1);
    assert_eq!(
        artifact::read_artifact(&connection).unwrap(),
        artifact::HistoryArtifact::History(HistoryKind::R)
    );
}

#[test]
fn opening_existing_metadata_less_store_does_not_backfill_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    drop(SqliteBackedHistory::with_file(path.clone(), None, None).unwrap());

    let store = HistoryStore::open(path.clone(), HistoryKind::R, None, None).unwrap();
    drop(store);

    let connection = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        artifact::read_artifact(&connection).unwrap(),
        artifact::HistoryArtifact::Legacy
    );
}

#[test]
fn opening_future_format_database_fails_before_reedline_opens_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.db");
    existing_database_with_metadata(&path, "history", "2", Some("r"));

    let error = HistoryStore::open(path, HistoryKind::R, None, None)
        .err()
        .expect("future format should be rejected");
    assert!(format!("{error:?}").contains("unsupported arf artifact format version 2"));
    assert_no_history_table(&dir.path().join("future.db"));
}

#[test]
fn opening_wrong_history_kind_fails_before_reedline_opens_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shell.db");
    existing_database_with_metadata(&path, "history", "1", Some("shell"));

    let error = HistoryStore::open(path, HistoryKind::R, None, None)
        .err()
        .expect("mismatched kind should be rejected");
    assert!(format!("{error:?}").contains("history database kind mismatch"));
    assert_no_history_table(&dir.path().join("shell.db"));
}

#[test]
fn opening_unified_export_as_history_fails_before_reedline_opens_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.db");
    existing_database_with_metadata(&path, "history-export", "1", None);

    let error = HistoryStore::open(path, HistoryKind::R, None, None)
        .err()
        .expect("unified export should be rejected");
    assert!(format!("{error:?}").contains("cannot open a unified history export"));
    assert_no_history_table(&dir.path().join("r.db"));
}

#[test]
fn concurrent_same_kind_first_opens_both_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.db");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

    let handles = [HistoryKind::R, HistoryKind::R].map(|kind| {
        let barrier = barrier.clone();
        let path = path.clone();
        std::thread::spawn(move || {
            barrier.wait();
            HistoryStore::open(path, kind, None, None)
        })
    });
    let stores: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().unwrap())
        .collect();

    assert_eq!(stores.len(), 2);
    assert_eq!(
        artifact::read_artifact_from_path(&path).unwrap(),
        artifact::HistoryArtifact::History(HistoryKind::R)
    );
}

#[test]
fn concurrent_different_kind_first_opens_have_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.db");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

    let handles = [HistoryKind::R, HistoryKind::Shell].map(|kind| {
        let barrier = barrier.clone();
        let path = path.clone();
        std::thread::spawn(move || {
            barrier.wait();
            (kind, HistoryStore::open(path, kind, None, None))
        })
    });
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    let successes: Vec<_> = results
        .iter()
        .filter_map(|(_, result)| result.as_ref().ok())
        .collect();
    let errors: Vec<_> = results
        .iter()
        .filter_map(|(_, result)| result.as_ref().err())
        .collect();

    assert_eq!(successes.len(), 1);
    assert_eq!(errors.len(), 1);
    assert!(format!("{:?}", errors[0]).contains("history database kind mismatch"));
    let (winner_kind, _) = results.iter().find(|(_, result)| result.is_ok()).unwrap();
    assert_eq!(
        artifact::read_artifact_from_path(&path).unwrap(),
        artifact::HistoryArtifact::History(*winner_kind)
    );
}

#[test]
fn configured_volatile_memory_failure_is_unavailable_without_previous_failure() {
    let runtime = HistoryRuntime::initialize_with_factories(
        &crate::config::HistoryMode::Volatile,
        None,
        HistoryKind::R,
        None,
        None,
        |_path, _kind, _session, _timestamp| injected_failure("persistent must not be opened"),
        |_session, _timestamp| injected_failure("configured memory failure"),
    );

    assert!(runtime.requested_path().is_none());
    let HistoryRuntime::Unavailable {
        failure,
        previous_failure,
    } = runtime
    else {
        panic!("configured volatile initialization should be unavailable");
    };
    assert_eq!(failure.stage, HistoryFailureStage::MemoryInitialization);
    assert!(failure.message.contains("configured memory failure"));
    assert!(previous_failure.is_none());
}

#[test]
fn persistent_failure_and_memory_failure_are_both_retained() {
    let requested_path = PathBuf::from("/requested/history.db");
    let runtime = HistoryRuntime::initialize_with_factories(
        &crate::config::HistoryMode::Persistent { dir: None },
        Some(requested_path.clone()),
        HistoryKind::R,
        None,
        None,
        |_path, _kind, _session, _timestamp| injected_failure("persistent open failure"),
        |_session, _timestamp| injected_failure("fallback memory failure"),
    );

    let detail = runtime.diagnostic_detail().expect("unavailable detail");
    assert_eq!(runtime.requested_path(), Some(requested_path.as_path()));
    let HistoryRuntime::Unavailable {
        failure,
        previous_failure,
    } = runtime
    else {
        panic!("persistent and fallback initialization should be unavailable");
    };
    assert_eq!(failure.stage, HistoryFailureStage::MemoryInitialization);
    assert!(failure.message.contains("fallback memory failure"));
    let previous_failure = previous_failure.expect("persistent failure should be retained");
    assert_eq!(previous_failure.stage, HistoryFailureStage::PersistentOpen);
    assert!(previous_failure.message.contains("persistent open failure"));
    assert!(detail.contains("fallback memory failure"));
    let requested_path_text = requested_path.to_string_lossy();
    assert_eq!(detail.matches(requested_path_text.as_ref()).count(), 0);
    let warning = HistoryRuntime::Unavailable {
        failure: HistoryFailureDetail::test_memory(),
        previous_failure: Some(HistoryFailureDetail::test_persistent_open(
            requested_path.clone(),
        )),
    }
    .startup_warning()
    .expect("unavailable warning");
    assert_eq!(warning.matches(requested_path_text.as_ref()).count(), 1);
}

#[test]
fn unknown_and_known_saves_have_expected_metadata() {
    let (_dir, store) = open_store();
    let unknown = store
        .save_unknown(HistoryItem::from_command_line(":not dispatched"))
        .unwrap();
    let known = store
        .save_known(
            HistoryItem::from_command_line("x <- 1:10"),
            HistoryExtraInfo::default(),
        )
        .unwrap();

    let unknown_typed = store
        .inner
        .lock()
        .unwrap()
        .load_with_extra::<HistoryExtraInfo>(unknown.id.unwrap())
        .unwrap();
    let known_typed = store
        .inner
        .lock()
        .unwrap()
        .load_with_extra::<HistoryExtraInfo>(known.id.unwrap())
        .unwrap();
    assert_eq!(unknown_typed.more_info, None);
    assert_eq!(known_typed.more_info, Some(HistoryExtraInfo::default()));
}

#[test]
fn strict_session_search_pages_before_applying_limit() {
    let current = reedline::Reedline::create_history_session_id().unwrap();
    let other = reedline::Reedline::create_history_session_id().unwrap();
    let store = HistoryStore::in_memory(Some(current), None).unwrap();

    for command in ["current old", "current new"] {
        let mut item = HistoryItem::from_command_line(command);
        item.session_id = Some(current);
        store.save_unknown(item).unwrap();
    }
    for index in 0..128 {
        let mut item = HistoryItem::from_command_line(format!("other {index}"));
        item.session_id = Some(other);
        store.save_unknown(item).unwrap();
    }

    let rows = store
        .search_strict_session(
            |start_id| SearchQuery {
                direction: reedline::SearchDirection::Backward,
                start_time: None,
                end_time: None,
                start_id,
                end_id: None,
                limit: None,
                filter: reedline::SearchFilter::anything(None),
            },
            Some(current),
            false,
            2,
            None,
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|item| item.session_id == Some(current)));
}

#[test]
fn strict_session_search_handles_future_time_without_rows() {
    let current = reedline::Reedline::create_history_session_id().unwrap();
    let store = HistoryStore::in_memory(Some(current), None).unwrap();
    let rows = store
        .search_strict_session(
            |start_id| SearchQuery {
                direction: reedline::SearchDirection::Backward,
                start_time: None,
                end_time: None,
                start_id,
                end_id: None,
                limit: None,
                filter: reedline::SearchFilter::anything(None),
            },
            Some(current),
            false,
            50,
            Some(
                chrono::DateTime::parse_from_rfc3339("2999-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            ),
        )
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn typed_finalization_preserves_future_fields() {
    let (_dir, store) = open_store();
    let item = store
        .save_known(
            HistoryItem::from_command_line("future"),
            serde_json::from_str(r#"{"future":true}"#).unwrap(),
        )
        .unwrap();
    store.finalize_meta_command(item.id.unwrap(), true).unwrap();
    let stored = store
        .inner
        .lock()
        .unwrap()
        .load_with_extra::<HistoryExtraInfo>(item.id.unwrap())
        .unwrap();
    let metadata = stored.more_info.unwrap();
    assert_eq!(metadata.meta_command(), Some(true));
    let fields: serde_json::Map<String, Value> =
        serde_json::from_str(&serde_json::to_string(&metadata).unwrap()).unwrap();
    assert_eq!(fields.get("future"), Some(&Value::Bool(true)));
}

#[test]
fn taking_a_recorded_outcome_consumes_it() {
    let receipt = HistorySaveReceipt::new();
    let outcome = HistorySaveOutcome::Saved(reedline::HistoryItemId::new(42));
    receipt.record(outcome);

    assert_eq!(receipt.take(), Some(outcome));
    assert_eq!(receipt.take(), None);
}

#[test]
fn failed_finalization_does_not_write_false() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let store = HistoryStore::open(path.clone(), HistoryKind::R, None, None).unwrap();
    let item = store
        .save_unknown(HistoryItem::from_command_line("will be cleared"))
        .unwrap();
    let id = item.id.unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            r#"CREATE TRIGGER fail_finalize
                   BEFORE UPDATE OF more_info ON history
                   BEGIN SELECT RAISE(ABORT, 'test failure'); END;"#,
        )
        .unwrap();
    assert!(store.finalize_meta_command(id, false).is_err());
    let connection = rusqlite::Connection::open(path).unwrap();
    let raw: Option<String> = connection
        .query_row(
            "SELECT more_info FROM history WHERE id = ?",
            [id.0],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, None);
}

#[test]
fn explicit_status_update_targets_the_outer_row_after_a_menu_row() {
    let (_dir, store) = open_store();
    let outer = store
        .save_unknown(HistoryItem::from_command_line("readline()"))
        .unwrap();
    let menu = store
        .save_unknown(HistoryItem::from_command_line(r#":menu choice"#))
        .unwrap();
    store.set_exit_status(outer.id.unwrap(), 1).unwrap();

    assert_eq!(store.load(outer.id.unwrap()).unwrap().exit_status, Some(1));
    assert_eq!(store.load(menu.id.unwrap()).unwrap().exit_status, None);
}
