//! History export functionality for backing up or transferring history.
//!
//! This module provides export functionality to create a unified SQLite file
//! containing both R and shell history. The exported file can be used for
//! backup or to transfer history to another machine.

use anyhow::{Context, Result, bail};
use std::path::Path;

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
    use rusqlite::{Connection, OpenFlags};

    // Validate table names to prevent SQL injection
    validate_table_name(r_table)?;
    validate_table_name(shell_table)?;

    // Ensure output file doesn't exist (don't overwrite)
    if output_path.exists() {
        bail!(
            "Output file already exists: {}\nRemove it or specify a different path.",
            output_path.display()
        );
    }

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            bail!(
                "Parent directory does not exist: {}\nCreate it first or specify a different path.",
                parent.display()
            );
        }
    }

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
            .with_context(|| format!("Failed to open shell history: {}", shell_db_path.display()))?;
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
        let mut insert_stmt = tx.prepare(&insert_sql).context("Failed to prepare insert")?;

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

        let result =
            export_history(&r_path, &temp_dir.path().join("nonexistent.db"), &output_path, "my_r", "my_shell")
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
        export_history(&r_path, &temp_dir.path().join("none.db"), &output_path, "r", "shell").unwrap();

        // Second export should fail
        let result = export_history(&r_path, &temp_dir.path().join("none.db"), &output_path, "r", "shell");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }
}
