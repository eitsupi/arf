//! History export functionality for backing up or transferring history.
//!
//! This module provides export functionality to create a unified SQLite file
//! containing both R and shell history. The exported file can be used for
//! backup or to transfer history to another machine.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Result of an export operation.
#[derive(Debug, Default)]
pub struct ExportResult {
    /// Number of R entries exported.
    pub r_exported: usize,
    /// Number of shell entries exported.
    pub shell_exported: usize,
}

/// Export history from r.db and shell.db to a unified SQLite file.
///
/// Creates a new SQLite file with tables named by `r_table` and `shell_table`,
/// each containing the full history schema (same as reedline's history table).
pub fn export_history(
    r_db_path: &Path,
    shell_db_path: &Path,
    output_path: &Path,
    r_table: &str,
    shell_table: &str,
) -> Result<ExportResult> {
    use super::import::validate_table_name;

    // Validate table names to prevent SQL injection
    validate_table_name(r_table)?;
    validate_table_name(shell_table)?;

    // Ensure the R and shell tables have different names
    if r_table == shell_table {
        bail!(
            "R table name and shell table name must be different (both are '{}')",
            r_table
        );
    }

    // Ensure output file doesn't exist (don't overwrite)
    if output_path.exists() {
        bail!(
            "Output file already exists: {}\nRemove it or specify a different path.",
            output_path.display()
        );
    }

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        bail!(
            "Parent directory does not exist: {}\nCreate it first or specify a different path.",
            parent.display()
        );
    }

    // Check write permission early by attempting to create and immediately remove a test file.
    // This gives a clearer error message than failing later during the actual write.
    let test_write_path = output_path.with_extension("arf-write-test");
    match fs::File::create(&test_write_path) {
        Ok(_) => {
            // Best-effort cleanup. If removal fails (e.g., due to race condition or
            // permission change), we continue anyway since write access was confirmed.
            // The .arf-write-test extension makes orphaned files identifiable.
            let _ = fs::remove_file(&test_write_path);
        }
        Err(e) => {
            bail!(
                "Cannot write to output location: {}\n{}",
                output_path.display(),
                e
            );
        }
    }

    // Use atomic write: write to temp file, then rename on success.
    // This prevents leaving incomplete files if export fails partway through.
    //
    // Note on atomicity: `fs::rename` is atomic on POSIX systems when source and
    // destination are on the same filesystem. On Windows, atomicity is not guaranteed
    // by the Win32 API. For single-user CLI usage, this is acceptable; concurrent
    // writes to the same output path are not a supported use case.
    //
    // The temp file includes timestamp and process ID for uniqueness:
    // - Timestamp prevents collisions across time
    // - Process ID prevents collisions from concurrent processes
    // - Together they avoid predictable paths on multi-user systems
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let temp_extension = format!("arf-export-tmp-{}-{}", timestamp, pid);
    let temp_path = output_path.with_extension(temp_extension);

    // Clean up any leftover temp file from a previous failed attempt.
    // Use unconditional remove to avoid TOCTOU race condition.
    match fs::remove_file(&temp_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| {
                format!("Failed to remove stale temp file: {}", temp_path.display())
            });
        }
    }

    // Perform export to temp file, with cleanup on failure
    let result = export_to_file(r_db_path, shell_db_path, &temp_path, r_table, shell_table);

    match result {
        Ok(export_result) => {
            // Atomically move temp file to final destination
            fs::rename(&temp_path, output_path).with_context(|| {
                format!(
                    "Failed to rename temp file {} to {}",
                    temp_path.display(),
                    output_path.display()
                )
            })?;
            Ok(export_result)
        }
        Err(e) => {
            // Clean up temp file on failure
            let _ = fs::remove_file(&temp_path);
            Err(e)
        }
    }
}

/// Internal function that performs the actual export to a file.
///
/// Note: This function allows exporting even when both source databases are missing,
/// returning an empty result with zero entries. The CLI handler in `main.rs` enforces
/// stricter validation (requiring at least one database to exist) to provide a better
/// user experience. This separation allows the low-level function to be more flexible
/// for potential future use cases (e.g., creating an empty export file template).
fn export_to_file(
    r_db_path: &Path,
    shell_db_path: &Path,
    output_path: &Path,
    r_table: &str,
    shell_table: &str,
) -> Result<ExportResult> {
    use rusqlite::{Connection, OpenFlags};

    // Create output database
    let mut output_db =
        Connection::open(output_path).context("Failed to create output database")?;

    let mut result = ExportResult::default();

    // Export R history if it exists
    if r_db_path.exists() {
        let r_db = Connection::open_with_flags(r_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Failed to open R history: {}", r_db_path.display()))?;
        result.r_exported = copy_history_table(&r_db, &mut output_db, r_table)?;
    }

    // Export shell history if it exists
    if shell_db_path.exists() {
        let shell_db = Connection::open_with_flags(shell_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| {
                format!("Failed to open shell history: {}", shell_db_path.display())
            })?;
        result.shell_exported = copy_history_table(&shell_db, &mut output_db, shell_table)?;
    }

    Ok(result)
}

/// Copy the history table from source to destination with a new table name.
fn copy_history_table(
    source: &rusqlite::Connection,
    dest: &mut rusqlite::Connection,
    dest_table: &str,
) -> Result<usize> {
    // Create the destination table with the same schema as reedline's history table
    let create_sql = format!(
        r#"CREATE TABLE IF NOT EXISTS "{}" (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            command_line TEXT NOT NULL,
            start_timestamp INTEGER,
            session_id INTEGER,
            hostname TEXT,
            cwd TEXT,
            duration_ms INTEGER,
            exit_status INTEGER,
            more_info TEXT
        )"#,
        dest_table
    );
    dest.execute(&create_sql, [])
        .with_context(|| format!("Failed to create table '{}'", dest_table))?;

    // Create indexes matching reedline's schema
    let index_sqls = [
        format!(
            r#"CREATE INDEX IF NOT EXISTS "idx_{}_time" ON "{}" (start_timestamp)"#,
            dest_table, dest_table
        ),
        format!(
            r#"CREATE INDEX IF NOT EXISTS "idx_{}_cwd" ON "{}" (cwd)"#,
            dest_table, dest_table
        ),
        format!(
            r#"CREATE INDEX IF NOT EXISTS "idx_{}_exit_status" ON "{}" (exit_status)"#,
            dest_table, dest_table
        ),
        format!(
            r#"CREATE INDEX IF NOT EXISTS "idx_{}_cmd" ON "{}" (command_line)"#,
            dest_table, dest_table
        ),
    ];
    for sql in &index_sqls {
        dest.execute(sql, []).context("Failed to create index")?;
    }

    // Check if source has history table
    let has_table: i32 = source
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='history'",
            [],
            |row| row.get(0),
        )
        .context("Failed to check if source has history table")?;

    if has_table == 0 {
        return Ok(0);
    }

    // Copy data from source
    let mut read_stmt = source
        .prepare(
            "SELECT command_line, start_timestamp, session_id, hostname, cwd, duration_ms, exit_status, more_info FROM history ORDER BY id",
        )
        .context("Failed to prepare read query")?;

    let insert_sql = format!(
        r#"INSERT INTO "{}" (command_line, start_timestamp, session_id, hostname, cwd, duration_ms, exit_status, more_info) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
        dest_table
    );

    let tx = dest.transaction().context("Failed to start transaction")?;
    let mut count = 0;

    {
        let mut insert_stmt = tx
            .prepare(&insert_sql)
            .context("Failed to prepare insert")?;

        let rows = read_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .context("Failed to query source history")?;

        for row in rows {
            let (cmd, ts, sess, host, cwd, dur, exit, info) =
                row.context("Failed to read row from source")?;
            insert_stmt
                .execute(rusqlite::params![cmd, ts, sess, host, cwd, dur, exit, info])
                .context("Failed to insert row")?;
            count += 1;
        }
    }

    tx.commit().context("Failed to commit transaction")?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::{History, HistoryItem, SqliteBackedHistory};
    use tempfile::TempDir;

    fn create_test_history(path: &Path, commands: &[&str]) {
        let mut history = SqliteBackedHistory::with_file(path.to_path_buf(), None, None).unwrap();
        for cmd in commands {
            history
                .save(HistoryItem {
                    id: None,
                    command_line: cmd.to_string(),
                    start_timestamp: None,
                    session_id: None,
                    hostname: None,
                    cwd: None,
                    duration: None,
                    exit_status: None,
                    more_info: None,
                })
                .unwrap();
        }
    }

    #[test]
    fn test_export_history_basic() {
        let temp_dir = TempDir::new().unwrap();
        let r_path = temp_dir.path().join("r.db");
        let shell_path = temp_dir.path().join("shell.db");
        let output_path = temp_dir.path().join("export.db");

        // Create test history databases
        create_test_history(&r_path, &["library(dplyr)", "print(1)"]);
        create_test_history(&shell_path, &["ls", "pwd"]);

        // Export
        let result = export_history(&r_path, &shell_path, &output_path, "r", "shell").unwrap();

        assert_eq!(result.r_exported, 2);
        assert_eq!(result.shell_exported, 2);
        assert!(output_path.exists());

        // Verify exported content
        let db = rusqlite::Connection::open(&output_path).unwrap();

        let r_count: i32 = db
            .query_row("SELECT COUNT(*) FROM r", [], |row| row.get(0))
            .unwrap();
        assert_eq!(r_count, 2);

        let shell_count: i32 = db
            .query_row("SELECT COUNT(*) FROM shell", [], |row| row.get(0))
            .unwrap();
        assert_eq!(shell_count, 2);
    }

    #[test]
    fn test_export_history_custom_table_names() {
        let temp_dir = TempDir::new().unwrap();
        let r_path = temp_dir.path().join("r.db");
        let output_path = temp_dir.path().join("export.db");

        create_test_history(&r_path, &["test"]);

        let result = export_history(
            &r_path,
            &temp_dir.path().join("nonexistent.db"),
            &output_path,
            "my_r",
            "my_shell",
        )
        .unwrap();

        assert_eq!(result.r_exported, 1);
        assert_eq!(result.shell_exported, 0);

        // Verify custom table name
        let db = rusqlite::Connection::open(&output_path).unwrap();
        let count: i32 = db
            .query_row("SELECT COUNT(*) FROM my_r", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_export_refuses_overwrite() {
        let temp_dir = TempDir::new().unwrap();
        let r_path = temp_dir.path().join("r.db");
        let output_path = temp_dir.path().join("export.db");

        create_test_history(&r_path, &["test"]);

        // First export
        export_history(
            &r_path,
            &temp_dir.path().join("none.db"),
            &output_path,
            "r",
            "shell",
        )
        .unwrap();

        // Second export should fail
        let result = export_history(
            &r_path,
            &temp_dir.path().join("none.db"),
            &output_path,
            "r",
            "shell",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_export_no_temp_file_left_on_success() {
        let temp_dir = TempDir::new().unwrap();
        let r_path = temp_dir.path().join("r.db");
        let output_path = temp_dir.path().join("export.db");

        create_test_history(&r_path, &["test"]);

        export_history(
            &r_path,
            &temp_dir.path().join("none.db"),
            &output_path,
            "r",
            "shell",
        )
        .unwrap();

        // Output file should exist, no temp files should remain
        assert!(output_path.exists());

        // Check that no temp files (with pattern arf-export-tmp-*) remain
        let entries: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("arf-export-tmp"))
            .collect();
        assert!(entries.is_empty(), "Temp files should be cleaned up");
    }

    #[test]
    fn test_export_rejects_same_table_names() {
        let temp_dir = TempDir::new().unwrap();
        let r_path = temp_dir.path().join("r.db");
        let output_path = temp_dir.path().join("export.db");

        create_test_history(&r_path, &["test"]);

        // Export with same table names should fail
        let result = export_history(
            &r_path,
            &temp_dir.path().join("none.db"),
            &output_path,
            "history",
            "history",
        );

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must be different"));
    }

    #[test]
    fn test_export_with_sqlite_reserved_words_as_table_names() {
        let temp_dir = TempDir::new().unwrap();
        let r_path = temp_dir.path().join("r.db");
        let shell_path = temp_dir.path().join("shell.db");
        let output_path = temp_dir.path().join("export.db");

        create_test_history(&r_path, &["library(dplyr)"]);
        create_test_history(&shell_path, &["ls"]);

        // SQLite reserved words should work when quoted
        let result = export_history(&r_path, &shell_path, &output_path, "select", "from").unwrap();

        assert_eq!(result.r_exported, 1);
        assert_eq!(result.shell_exported, 1);

        // Verify we can read back the data
        let db = rusqlite::Connection::open(&output_path).unwrap();
        let r_count: i32 = db
            .query_row(r#"SELECT COUNT(*) FROM "select""#, [], |row| row.get(0))
            .unwrap();
        let shell_count: i32 = db
            .query_row(r#"SELECT COUNT(*) FROM "from""#, [], |row| row.get(0))
            .unwrap();
        assert_eq!(r_count, 1);
        assert_eq!(shell_count, 1);
    }

    #[test]
    fn test_export_cleans_up_temp_file_on_failure() {
        let temp_dir = TempDir::new().unwrap();
        // Point to a corrupted/invalid database file.
        // The export flow is:
        // 1. export_history creates temp file path
        // 2. export_to_file opens output temp file (SQLite creates it)
        // 3. export_to_file tries to open source r.db -> FAILS here because invalid
        // 4. export_history catches error and removes temp file
        let invalid_db_path = temp_dir.path().join("invalid.db");
        std::fs::write(&invalid_db_path, "not a valid sqlite database").unwrap();

        let output_path = temp_dir.path().join("export.db");

        // Export should fail because the source database is invalid
        let result = export_history(
            &invalid_db_path,
            &temp_dir.path().join("none.db"),
            &output_path,
            "r",
            "shell",
        );

        assert!(result.is_err());

        // No output file should exist
        assert!(!output_path.exists());

        // No temp files should remain (cleanup should have removed the temp file
        // that was created by SQLite before the source database open failed)
        let entries: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("arf-export-tmp"))
            .collect();
        assert!(
            entries.is_empty(),
            "Temp files should be cleaned up on failure"
        );
    }

    #[test]
    fn test_export_cleans_up_temp_file_on_copy_failure() {
        // This test verifies cleanup when failure occurs DURING data copy,
        // not just during database open. This ensures the temp file was
        // definitely created and contains partial data before cleanup.
        let temp_dir = TempDir::new().unwrap();

        // Create a valid source database
        let r_path = temp_dir.path().join("r.db");
        create_test_history(&r_path, &["test1", "test2"]);

        // Create a shell database file that exists but is invalid
        // This causes failure during shell history copy (after R history succeeds)
        let shell_path = temp_dir.path().join("shell.db");
        std::fs::write(&shell_path, "not a valid sqlite database").unwrap();

        let output_path = temp_dir.path().join("export.db");

        // Export should fail when trying to open the invalid shell database
        let result = export_history(&r_path, &shell_path, &output_path, "r", "shell");

        assert!(result.is_err());

        // No output file should exist
        assert!(!output_path.exists());

        // No temp files should remain
        let entries: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("arf-export-tmp"))
            .collect();
        assert!(
            entries.is_empty(),
            "Temp files should be cleaned up on copy failure"
        );
    }

    #[test]
    fn test_export_import_round_trip() {
        use crate::history::import::{ImportTargets, import_entries, parse_unified_arf_history};
        use reedline::History;

        let temp_dir = TempDir::new().unwrap();

        // Create source history databases with test data
        let r_path = temp_dir.path().join("r.db");
        let shell_path = temp_dir.path().join("shell.db");
        create_test_history(&r_path, &["library(dplyr)", "summary(iris)", "print(1)"]);
        create_test_history(&shell_path, &["ls -la", "pwd", "git status"]);

        // Export to a unified file
        let export_path = temp_dir.path().join("backup.db");
        let export_result =
            export_history(&r_path, &shell_path, &export_path, "r", "shell").unwrap();

        assert_eq!(export_result.r_exported, 3);
        assert_eq!(export_result.shell_exported, 3);

        // Parse the exported file
        let entries = parse_unified_arf_history(&export_path, "r", "shell").unwrap();
        assert_eq!(entries.len(), 6);

        // Import into new databases
        let new_r_path = temp_dir.path().join("new_r.db");
        let new_shell_path = temp_dir.path().join("new_shell.db");
        let mut targets = ImportTargets {
            r_history: SqliteBackedHistory::with_file(new_r_path, None, None).unwrap(),
            shell_history: SqliteBackedHistory::with_file(new_shell_path, None, None).unwrap(),
        };

        let import_result = import_entries(&mut targets, entries, None, false).unwrap();
        assert_eq!(import_result.r_imported, 3);
        assert_eq!(import_result.shell_imported, 3);

        // Verify the imported data matches the original
        let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Forward, None);
        let r_items = targets.r_history.search(r_query).unwrap();
        let r_commands: Vec<&str> = r_items.iter().map(|i| i.command_line.as_str()).collect();
        assert!(r_commands.contains(&"library(dplyr)"));
        assert!(r_commands.contains(&"summary(iris)"));
        assert!(r_commands.contains(&"print(1)"));

        let shell_query =
            reedline::SearchQuery::everything(reedline::SearchDirection::Forward, None);
        let shell_items = targets.shell_history.search(shell_query).unwrap();
        let shell_commands: Vec<&str> = shell_items
            .iter()
            .map(|i| i.command_line.as_str())
            .collect();
        assert!(shell_commands.contains(&"ls -la"));
        assert!(shell_commands.contains(&"pwd"));
        assert!(shell_commands.contains(&"git status"));
    }
}
