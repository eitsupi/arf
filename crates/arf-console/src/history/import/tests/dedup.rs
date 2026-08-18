use super::super::*;
use super::create_test_targets;

// === Dedup (anti-join) tests ===

#[test]
fn test_import_skips_duplicates_with_timestamp() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let ts = DateTime::parse_from_rfc3339("2024-06-15T14:30:45Z")
        .unwrap()
        .with_timezone(&Utc);

    // First import: should succeed
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 1);
    assert_eq!(result.duplicates_skipped, 0);

    // Second import of the same entry: should be skipped as duplicate
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 0);
    assert_eq!(result.duplicates_skipped, 1);

    // Verify only one entry exists in the database
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 1);
}

#[test]
fn test_import_skips_duplicates_without_timestamp() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    // First import (no timestamp)
    let entries = vec![ImportEntry {
        command: "summary(iris)".to_string(),
        timestamp: None,
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 1);
    assert_eq!(result.duplicates_skipped, 0);

    // Second import of the same command (no timestamp): should be skipped
    let entries = vec![ImportEntry {
        command: "summary(iris)".to_string(),
        timestamp: None,
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 0);
    assert_eq!(result.duplicates_skipped, 1);

    // Verify only one entry exists
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 1);
}

#[test]
fn test_import_allows_same_command_different_timestamp() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let ts1 = DateTime::parse_from_rfc3339("2024-06-15T14:30:45Z")
        .unwrap()
        .with_timezone(&Utc);
    let ts2 = DateTime::parse_from_rfc3339("2024-06-15T15:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    // Import the same command with two different timestamps
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts1),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 1);

    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts2),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 1);
    assert_eq!(result.duplicates_skipped, 0);

    // Both should exist
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 2);
}

#[test]
fn test_import_duplicates_flag_disables_dedup() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let ts = DateTime::parse_from_rfc3339("2024-06-15T14:30:45Z")
        .unwrap()
        .with_timezone(&Utc);

    // First import
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, false).unwrap();
    assert_eq!(result.r_imported, 1);

    // Second import with skip_duplicates=false (--import-duplicates)
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, false).unwrap();
    assert_eq!(result.r_imported, 1);
    assert_eq!(result.duplicates_skipped, 0);

    // Both entries should exist (duplicate allowed)
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 2);
}

#[test]
fn test_import_dedup_works_per_database() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    // Import an R command
    let entries = vec![ImportEntry {
        command: "ls -la".to_string(),
        timestamp: None,
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 1);

    // Import the same command as shell — should NOT be a duplicate
    // because it's checked against the shell database, not R
    let entries = vec![ImportEntry {
        command: "ls -la".to_string(),
        timestamp: None,
        mode: Some("shell".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.shell_imported, 1);
    assert_eq!(result.duplicates_skipped, 0);

    // Verify both databases have the entry
    let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let r_items = targets.r_history.search(r_query).unwrap();
    assert_eq!(r_items.len(), 1);

    let shell_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let shell_items = targets.shell_history.search(shell_query).unwrap();
    assert_eq!(shell_items.len(), 1);
}

#[test]
fn test_import_dry_run_with_dedup() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let ts = DateTime::parse_from_rfc3339("2024-06-15T14:30:45Z")
        .unwrap()
        .with_timezone(&Utc);

    // Pre-populate the database
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    import_entries(&mut targets, entries, None, false).unwrap();

    // Build dedup sets
    let r_dedup = DedupSet::from_history(&targets.r_history).unwrap();
    let shell_dedup = DedupSet::from_history(&targets.shell_history).unwrap();

    // Dry run with existing + new entries
    let entries = vec![
        ImportEntry {
            command: "library(dplyr)".to_string(), // duplicate
            timestamp: Some(ts),
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "print(1)".to_string(), // new
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
    ];

    let result = import_entries_dry_run(&entries, Some(&r_dedup), Some(&shell_dedup));
    assert_eq!(result.r_imported, 1);
    assert_eq!(result.duplicates_skipped, 1);
}

#[test]
fn test_import_mixed_dedup_new_and_existing() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let ts = DateTime::parse_from_rfc3339("2024-06-15T14:30:45Z")
        .unwrap()
        .with_timezone(&Utc);

    // Pre-populate with one entry
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    import_entries(&mut targets, entries, None, false).unwrap();

    // Import a batch with duplicates and new entries
    let entries = vec![
        ImportEntry {
            command: "library(dplyr)".to_string(), // duplicate
            timestamp: Some(ts),
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "print(1)".to_string(), // new
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "git status".to_string(), // new (shell)
            timestamp: None,
            mode: Some("shell".to_string()),
            metadata: None,
        },
    ];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 1);
    assert_eq!(result.shell_imported, 1);
    assert_eq!(result.duplicates_skipped, 1);

    // Verify databases
    let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let r_items = targets.r_history.search(r_query).unwrap();
    assert_eq!(r_items.len(), 2); // original + new

    let shell_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let shell_items = targets.shell_history.search(shell_query).unwrap();
    assert_eq!(shell_items.len(), 1);
}

#[test]
fn test_import_dry_run_with_partial_dedup() {
    // Regression test: dry-run dedup should work when only one database
    // has a dedup set (e.g., r.db exists but shell.db doesn't).
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    // Pre-populate only the R database
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: None,
        mode: Some("r".to_string()),
        metadata: None,
    }];
    import_entries(&mut targets, entries, None, false).unwrap();

    // Build dedup set only for R (simulating shell.db not existing)
    let r_dedup = DedupSet::from_history(&targets.r_history).unwrap();

    let entries = vec![
        ImportEntry {
            command: "library(dplyr)".to_string(), // duplicate in R
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "print(1)".to_string(), // new R entry
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "ls -la".to_string(), // shell entry, no dedup set
            timestamp: None,
            mode: Some("shell".to_string()),
            metadata: None,
        },
    ];

    // Pass R dedup but None for shell
    let result = import_entries_dry_run(&entries, Some(&r_dedup), None);
    assert_eq!(result.r_imported, 1); // only "print(1)"
    assert_eq!(result.shell_imported, 1); // "ls -la" not checked (no shell dedup)
    assert_eq!(result.duplicates_skipped, 1); // "library(dplyr)"
}

#[test]
fn test_import_skips_notimestamp_when_timestamped_exists() {
    // Regression test: a no-timestamp import entry should be skipped if
    // the same command already exists in the DB with any timestamp.
    // This is documented in lines 265-270.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let ts = DateTime::parse_from_rfc3339("2024-06-15T14:30:45Z")
        .unwrap()
        .with_timezone(&Utc);

    // Pre-populate with a timestamped entry
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: Some(ts),
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 1);

    // Try to import the same command without a timestamp: should be skipped
    let entries = vec![ImportEntry {
        command: "library(dplyr)".to_string(),
        timestamp: None,
        mode: Some("r".to_string()),
        metadata: None,
    }];
    let result = import_entries(&mut targets, entries, None, true).unwrap();
    assert_eq!(result.r_imported, 0);
    assert_eq!(result.duplicates_skipped, 1);

    // Verify only the original entry exists
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = targets.r_history.search(query).unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].start_timestamp.is_some());
}

#[test]
fn test_from_db_matches_from_history() {
    // Verify that from_db (read-only SQLite) and from_history (via reedline)
    // produce the same dedup set for the same database contents.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);

    let ts1 = Utc::now();
    let ts2 = ts1 + chrono::Duration::seconds(60);

    let entries = vec![
        ImportEntry {
            command: "library(dplyr)".to_string(),
            timestamp: Some(ts1),
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "print(1)".to_string(),
            timestamp: Some(ts2),
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "summary(iris)".to_string(),
            timestamp: None, // no timestamp
            mode: Some("r".to_string()),
            metadata: None,
        },
    ];
    import_entries(&mut targets, entries, None, false).unwrap();

    let r_path = temp_dir.path().join("r.db");
    let from_history = DedupSet::from_history(&targets.r_history).unwrap();
    let from_db = DedupSet::from_db(&r_path).unwrap();

    // Both should have the same commands set
    assert_eq!(from_history.commands, from_db.commands);
    // Both should have the same command_timestamps set
    assert_eq!(from_history.command_timestamps, from_db.command_timestamps);

    // Verify dedup behavior is identical for both
    assert!(from_history.is_duplicate("library(dplyr)", Some(&ts1)));
    assert!(from_db.is_duplicate("library(dplyr)", Some(&ts1)));
    assert!(!from_history.is_duplicate("new_cmd", None));
    assert!(!from_db.is_duplicate("new_cmd", None));
    assert!(from_history.is_duplicate("summary(iris)", None));
    assert!(from_db.is_duplicate("summary(iris)", None));
}

fn test_metadata(value: &str) -> HistoryExtraInfo {
    serde_json::from_str(value).unwrap()
}

#[test]
fn test_metadata_backfills_duplicate_without_metadata() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);
    let command = "backfill me";
    import_entries(
        &mut targets,
        vec![ImportEntry {
            command: command.to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        }],
        None,
        false,
    )
    .unwrap();

    let metadata = test_metadata(r#"{"future":true}"#);
    let result = import_entries(
        &mut targets,
        vec![ImportEntry {
            command: command.to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: Some(metadata.clone()),
        }],
        None,
        true,
    )
    .unwrap();
    assert_eq!(result.r_imported, 0);
    assert_eq!(result.metadata_backfilled, 1);
    assert_eq!(result.duplicates_skipped, 0);
    assert_eq!(
        targets
            .r_history
            .load_with_metadata(reedline::HistoryItemId::new(1))
            .unwrap()
            .more_info,
        Some(metadata)
    );
}

#[test]
fn test_existing_metadata_wins_on_duplicate() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);
    let original = test_metadata(r#"{"owner":"target"}"#);
    import_entries(
        &mut targets,
        vec![ImportEntry {
            command: "target wins".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: Some(original.clone()),
        }],
        None,
        false,
    )
    .unwrap();

    let result = import_entries(
        &mut targets,
        vec![ImportEntry {
            command: "target wins".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: Some(test_metadata(r#"{"owner":"source"}"#)),
        }],
        None,
        true,
    )
    .unwrap();
    assert_eq!(result.metadata_backfilled, 0);
    assert_eq!(result.duplicates_skipped, 1);
    assert_eq!(
        targets
            .r_history
            .load_with_metadata(reedline::HistoryItemId::new(1))
            .unwrap()
            .more_info,
        Some(original)
    );
}

#[test]
fn test_no_timestamp_metadata_duplicate_with_multiple_matches_warns() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut targets = create_test_targets(&temp_dir);
    let entries = vec![
        ImportEntry {
            command: "ambiguous".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
        ImportEntry {
            command: "ambiguous".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: None,
        },
    ];
    import_entries(&mut targets, entries, None, false).unwrap();

    let result = import_entries(
        &mut targets,
        vec![ImportEntry {
            command: "ambiguous".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: Some(test_metadata(r#"{"future":true}"#)),
        }],
        None,
        true,
    )
    .unwrap();
    assert_eq!(result.metadata_backfilled, 0);
    assert_eq!(result.duplicates_skipped, 1);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("multiple rows"));
    let rows = targets
        .r_history
        .search(reedline::SearchQuery::everything(
            reedline::SearchDirection::Backward,
            None,
        ))
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn test_dry_run_counts_metadata_backfill_without_writing() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("r.db");
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
        "INSERT INTO history (command_line) VALUES (?)",
        ["dry backfill"],
    )
    .unwrap();
    drop(db);

    let dedup = DedupSet::from_db(&path).unwrap();
    let result = import_entries_dry_run(
        &[ImportEntry {
            command: "dry backfill".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
            metadata: Some(test_metadata(r#"{"future":true}"#)),
        }],
        Some(&dedup),
        None,
    );
    assert_eq!(result.r_imported, 0);
    assert_eq!(result.metadata_backfilled, 1);
    assert!(!path.with_extension("db-wal").exists());
    assert!(!path.with_extension("db-shm").exists());
}
