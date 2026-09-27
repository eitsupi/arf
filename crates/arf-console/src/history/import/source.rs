//! Parsing and reading history from external sources.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDateTime, Utc};
use reedline::{HistoryItem, HistoryItemId, HistorySessionId};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::metadata::HistoryExtraInfo;
use super::{ImportEntry, ImportMode, ParsedImport};

/// Get the default radian history file path.
pub fn default_radian_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".radian_history"))
        .unwrap_or_else(|| PathBuf::from(".radian_history"))
}

/// Get the default R history file path.
///
/// Checks R_HISTFILE environment variable first, then falls back to .Rhistory
/// in the current directory.
pub fn default_r_history_path() -> PathBuf {
    if let Ok(path) = std::env::var("R_HISTFILE") {
        return PathBuf::from(path);
    }
    PathBuf::from(".Rhistory")
}

/// Parse a radian history file.
///
/// The radian format uses:
/// - `# time: YYYY-MM-DD HH:MM:SS UTC` for timestamps
/// - `# mode: <mode>` for the input mode
/// - `+<line>` for command lines (may span multiple lines)
/// - Blank lines separate entries
pub fn parse_radian_history(path: &Path) -> Result<ParsedImport> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open radian history: {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut entries = Vec::new();
    let mut current_timestamp: Option<DateTime<Utc>> = None;
    let mut current_mode: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line_result in reader.lines() {
        let line = line_result.with_context(|| "Failed to read line from radian history")?;

        if line.starts_with("# time: ") {
            // Finalize previous entry if we have one
            if !current_lines.is_empty() {
                let command = current_lines.join("\n");
                let mut entry = ImportEntry::new(command)
                    .with_mode(ImportMode::from_external(current_mode.take().as_deref()));
                entry.item.start_timestamp = current_timestamp;
                entries.push(entry);
                current_lines.clear();
            }

            // Reset mode on new timestamp boundary to prevent carryover
            // (e.g., if previous entry had "# mode: shell" but new entry has no mode line)
            current_mode = None;

            // Parse timestamp: "# time: 2024-01-15 10:30:00 UTC"
            let time_str = line.trim_start_matches("# time: ").trim();
            let time_str = time_str.trim_end_matches(" UTC");
            current_timestamp = NaiveDateTime::parse_from_str(time_str, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|naive| naive.and_utc());
        } else if line.starts_with("# mode: ") {
            current_mode = Some(line.trim_start_matches("# mode: ").trim().to_string());
        } else if let Some(content) = line.strip_prefix('+') {
            // Handle CRLF line endings - strip trailing \r
            let content = content.strip_suffix('\r').unwrap_or(content);
            current_lines.push(content.to_string());
        } else if line.trim().is_empty() {
            // Empty line can separate entries
            if !current_lines.is_empty() {
                let command = current_lines.join("\n");
                let mut entry = ImportEntry::new(command)
                    .with_mode(ImportMode::from_external(current_mode.take().as_deref()));
                entry.item.start_timestamp = current_timestamp;
                entries.push(entry);
                current_lines.clear();
                current_timestamp = None;
            }
        }
        // Ignore other lines (comments, etc.)
    }

    // Don't forget the last entry
    if !current_lines.is_empty() {
        let command = current_lines.join("\n");
        let mut entry = ImportEntry::new(command)
            .with_mode(ImportMode::from_external(current_mode.take().as_deref()));
        entry.item.start_timestamp = current_timestamp;
        entries.push(entry);
    }

    Ok(ParsedImport {
        entries,
        warnings: Vec::new(),
    })
}

/// Parse an R native history file (.Rhistory).
///
/// The R native format is simply one command per line, no metadata.
/// Multi-line commands are NOT supported by R's native history.
pub fn parse_r_history(path: &Path) -> Result<ParsedImport> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open R history: {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut entries = Vec::new();

    for line_result in reader.lines() {
        let line = line_result.with_context(|| "Failed to read line from R history")?;
        // Only trim line endings, preserve leading whitespace (e.g., indented code)
        let content = line.trim_end();
        // Skip empty/whitespace-only lines
        if !content.trim().is_empty() {
            entries.push(ImportEntry::new(content.to_string()).with_mode(ImportMode::R));
        }
    }

    Ok(ParsedImport {
        entries,
        warnings: Vec::new(),
    })
}

/// Copy entries from another arf SQLite history database.
///
/// Versioned arf metadata determines the history kind. For metadata-less legacy
/// databases, the mode is inferred from the filename: `shell.db` is shell history
/// and all other names are treated as R history.
pub fn parse_arf_history(path: &Path) -> Result<ParsedImport> {
    if !path.exists() {
        bail!("arf history database not found: {}", path.display());
    }

    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Failed to open arf history database: {}", path.display()))?;
    let mode = match super::super::artifact::read_artifact(&db)? {
        super::super::artifact::HistoryArtifact::History(
            super::super::artifact::HistoryKind::R,
        ) => ImportMode::R,
        super::super::artifact::HistoryArtifact::History(
            super::super::artifact::HistoryKind::Shell,
        ) => ImportMode::Shell,
        super::super::artifact::HistoryArtifact::Export => {
            bail!(
                "File '{}' is a unified history export, not a single history database",
                path.display()
            )
        }
        super::super::artifact::HistoryArtifact::Legacy => {
            if path.file_name().and_then(|name| name.to_str()) == Some("shell.db") {
                ImportMode::Shell
            } else {
                ImportMode::R
            }
        }
    };
    if !table_exists(&db, "history")? {
        bail!(
            "File '{}' does not look like an arf history database: missing history table",
            path.display()
        );
    }

    read_history_table(&db, path, "history", mode).with_context(|| {
        format!(
            "File '{}' does not look like an arf history database",
            path.display()
        )
    })
}

/// Validate that a table name is safe for use in SQL queries.
///
/// Table names must contain only alphanumeric characters and underscores,
/// must not be empty, and must contain at least one alphanumeric character.
/// This prevents SQL injection attacks and avoids confusing names like `_` or `___`.
pub fn validate_table_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Table name cannot be empty");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!(
            "Invalid table name '{}': must contain only alphanumeric characters and underscores",
            name
        );
    }
    // SQLite identifiers cannot start with a digit
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        bail!("Invalid table name '{}': cannot start with a digit", name);
    }
    // Require at least one alphanumeric character to avoid confusing names like "_" or "___"
    if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
        bail!(
            "Invalid table name '{}': must contain at least one alphanumeric character",
            name
        );
    }
    Ok(())
}

/// Parse entries from a unified arf export file that contains both R and shell history.
///
/// This function reads from a SQLite file that has separate tables for R and shell history,
/// as created by `export_history`. The table names are specified by the caller.
/// At least one configured table must exist; a missing individual table is skipped.
///
pub fn parse_unified_arf_history(
    path: &Path,
    r_table: &str,
    shell_table: &str,
) -> Result<ParsedImport> {
    use rusqlite::{Connection, OpenFlags};

    // Validate table names to prevent SQL injection
    validate_table_name(r_table)?;
    validate_table_name(shell_table)?;

    // Ensure the R and shell tables have different names to avoid duplicate entries
    if r_table == shell_table {
        bail!(
            "R table name and shell table name must be different (both are '{}')",
            r_table
        );
    }

    if !path.exists() {
        bail!("arf export file not found: {}", path.display());
    }

    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("Failed to open arf export file: {}", path.display()))?;
    match super::super::artifact::read_artifact(&db)? {
        super::super::artifact::HistoryArtifact::Legacy
        | super::super::artifact::HistoryArtifact::Export => {}
        super::super::artifact::HistoryArtifact::History(_) => {
            bail!(
                "File '{}' is a single history database, not a unified history export",
                path.display()
            )
        }
    }

    let has_r_table = table_exists(&db, r_table)?;
    let has_shell_table = table_exists(&db, shell_table)?;
    if !has_r_table && !has_shell_table {
        bail!(
            "File '{}' does not look like an arf export: missing configured history tables '{}' and '{}'",
            path.display(),
            r_table,
            shell_table
        );
    }

    let mut parsed = ParsedImport::default();

    // Try to read R history table
    if has_r_table {
        let r_entries = read_history_table(&db, path, r_table, ImportMode::R)?;
        parsed.entries.extend(r_entries.entries);
        parsed.warnings.extend(r_entries.warnings);
    }

    // Try to read shell history table
    if has_shell_table {
        let shell_entries = read_history_table(&db, path, shell_table, ImportMode::Shell)?;
        parsed.entries.extend(shell_entries.entries);
        parsed.warnings.extend(shell_entries.warnings);
    }

    Ok(parsed)
}

/// Check if a table exists in the database.
fn table_exists(db: &rusqlite::Connection, table_name: &str) -> Result<bool> {
    let count: i32 = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            [table_name],
            |row| row.get(0),
        )
        .context("Failed to check if table exists")?;
    Ok(count > 0)
}

/// Read history entries from a table.
fn read_history_table(
    db: &rusqlite::Connection,
    source_path: &Path,
    table_name: &str,
    mode: ImportMode,
) -> Result<ParsedImport> {
    use chrono::TimeZone;

    // Use format! for table name since it can't be parameterized in SQL.
    // Table names are validated by validate_table_name() before reaching here.
    let columns = HistoryTableColumns::read(db, table_name)?;
    let query = format!(
        r#"SELECT id, command_line, start_timestamp, {}, {}, {}, {}, {}, {} FROM "{}" ORDER BY id"#,
        columns.expression("session_id"),
        columns.expression("hostname"),
        columns.expression("cwd"),
        columns.expression("duration_ms"),
        columns.expression("exit_status"),
        columns.expression("more_info"),
        table_name
    );

    let mut stmt = db.prepare(&query).with_context(|| {
        format!(
            "Failed to query table '{}' (not a valid history table?)",
            table_name
        )
    })?;

    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let command: String = row.get(1)?;
            let ts_millis: Option<i64> = row.get(2)?;
            let session_id: Option<i64> = row.get(3)?;
            let hostname: Option<String> = row.get(4)?;
            let cwd: Option<String> = row.get(5)?;
            let duration_millis: Option<i64> = row.get(6)?;
            let exit_status: Option<i64> = row.get(7)?;
            let raw_metadata: Option<String> = row.get(8)?;
            Ok((
                id,
                command,
                ts_millis,
                session_id,
                hostname,
                cwd,
                duration_millis,
                exit_status,
                raw_metadata,
            ))
        })
        .context("Failed to query history")?;

    let mut parsed = ParsedImport::default();
    for row in rows {
        let (
            id,
            command,
            ts_millis,
            raw_session_id,
            hostname,
            cwd,
            duration_millis,
            exit_status,
            raw_metadata,
        ) = row.context("Failed to read history row")?;
        let timestamp = ts_millis.and_then(|ms| Utc.timestamp_millis_opt(ms).single());
        let session_id = raw_session_id
            .map(|id| serde_json::from_str::<HistorySessionId>(&id.to_string()))
            .transpose()
            .with_context(|| format!("Invalid session_id in row {}", id))?;
        let duration = match duration_millis {
            Some(ms) if ms >= 0 => Some(Duration::from_millis(ms as u64)),
            Some(ms) => {
                parsed.warnings.push(format!(
                    "Invalid negative duration {} for row {} from '{}'; importing with NULL duration",
                    ms,
                    id,
                    source_path.display()
                ));
                None
            }
            None => None,
        };
        let metadata = parse_row_metadata(
            raw_metadata.as_deref(),
            source_path,
            HistoryItemId::new(id),
            &mut parsed.warnings,
        );
        parsed.entries.push(ImportEntry {
            mode: mode.clone(),
            item: HistoryItem {
                id: None,
                start_timestamp: timestamp,
                command_line: command,
                session_id,
                hostname,
                cwd,
                duration,
                exit_status,
                more_info: metadata,
            },
        });
    }

    Ok(parsed)
}

pub(super) struct HistoryTableColumns {
    names: HashSet<String>,
}

impl HistoryTableColumns {
    pub(super) fn read(db: &rusqlite::Connection, table_name: &str) -> Result<Self> {
        let mut names = HashSet::new();
        let query = format!(r#"PRAGMA table_info("{}")"#, table_name);
        let mut stmt = db
            .prepare(&query)
            .context("Failed to inspect history table")?;
        let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for column in columns {
            names.insert(column?);
        }
        Ok(Self { names })
    }

    pub(super) fn expression(&self, column: &str) -> String {
        if column == "more_info" {
            if self.names.contains(column) {
                "CAST(more_info AS TEXT)".to_string()
            } else {
                "NULL".to_string()
            }
        } else if self.names.contains(column) {
            column.to_string()
        } else {
            "NULL".to_string()
        }
    }
}

fn parse_row_metadata(
    raw_metadata: Option<&str>,
    source: &Path,
    id: HistoryItemId,
    warnings: &mut Vec<String>,
) -> Option<HistoryExtraInfo> {
    let raw_metadata = raw_metadata?;
    match serde_json::from_str::<HistoryExtraInfo>(raw_metadata) {
        Ok(metadata) => Some(metadata),
        Err(error) => {
            warnings.push(format!(
                "Could not deserialize metadata for row {} from '{}': {}; importing with NULL metadata",
                id.0,
                source.display(),
                error
            ));
            None
        }
    }
}
