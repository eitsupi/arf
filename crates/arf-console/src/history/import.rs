//! History import functionality for migrating from other R environments.
//!
//! This module provides importers for:
//! - **radian**: Parse `~/.radian_history` format with timestamps and modes
//! - **R native**: Parse `.Rhistory` plain text format
//! - **arf**: Copy from another arf SQLite database
//!
//! # Radian History Format
//!
//! ```text
//! # time: 2024-01-15 10:30:00 UTC
//! # mode: r
//! +library(dplyr)
//! +iris %>%
//! +  filter(Species == "setosa")
//!
//! # time: 2024-01-15 10:31:00 UTC
//! # mode: shell
//! +ls -la
//! ```
//!
//! # R Native History Format
//!
//! Simple text file with one command per line (no metadata):
//! ```text
//! library(dplyr)
//! print("hello")
//! ```

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDateTime, Utc};
use reedline::{HistoryItem, SqliteBackedHistory};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// A parsed history entry ready for import.
#[derive(Debug, Clone)]
pub struct ImportEntry {
    /// The command text.
    pub command: String,
    /// Timestamp when the command was executed (if available).
    pub timestamp: Option<DateTime<Utc>>,
    /// Mode in which the command was executed (r, shell, browse).
    pub mode: Option<String>,
}

/// Result of an import operation.
#[derive(Debug, Default)]
pub struct ImportResult {
    /// Number of R entries successfully imported.
    pub r_imported: usize,
    /// Number of shell entries successfully imported.
    pub shell_imported: usize,
    /// Number of entries skipped (empty, unknown mode, errors).
    pub skipped: usize,
    /// Number of duplicate entries skipped.
    pub duplicates_skipped: usize,
    /// Warning messages for non-fatal issues.
    pub warnings: Vec<String>,
}

impl ImportResult {
    /// Total number of entries imported.
    #[allow(dead_code)]
    pub fn total_imported(&self) -> usize {
        self.r_imported + self.shell_imported
    }
}

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
pub fn parse_radian_history(path: &Path) -> Result<Vec<ImportEntry>> {
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
                entries.push(ImportEntry {
                    command,
                    timestamp: current_timestamp,
                    mode: current_mode.take(),
                });
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
                entries.push(ImportEntry {
                    command,
                    timestamp: current_timestamp,
                    mode: current_mode.take(),
                });
                current_lines.clear();
                current_timestamp = None;
            }
        }
        // Ignore other lines (comments, etc.)
    }

    // Don't forget the last entry
    if !current_lines.is_empty() {
        let command = current_lines.join("\n");
        entries.push(ImportEntry {
            command,
            timestamp: current_timestamp,
            mode: current_mode.take(),
        });
    }

    Ok(entries)
}

/// Parse an R native history file (.Rhistory).
///
/// The R native format is simply one command per line, no metadata.
/// Multi-line commands are NOT supported by R's native history.
pub fn parse_r_history(path: &Path) -> Result<Vec<ImportEntry>> {
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
            entries.push(ImportEntry {
                command: content.to_string(),
                timestamp: None,
                mode: Some("r".to_string()),
            });
        }
    }

    Ok(entries)
}

/// Copy entries from another arf SQLite history database.
///
/// The mode is inferred from the filename:
/// - Files named `shell.db` are treated as shell history
/// - All other files are treated as R history
pub fn parse_arf_history(path: &Path) -> Result<Vec<ImportEntry>> {
    use reedline::History;

    if !path.exists() {
        bail!("arf history database not found: {}", path.display());
    }

    // Infer mode from filename
    let is_shell = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "shell.db");
    let mode = if is_shell {
        Some("shell".to_string())
    } else {
        Some("r".to_string())
    };

    // Open source history database
    let source = SqliteBackedHistory::with_file(path.to_path_buf(), None, None)
        .with_context(|| format!("Failed to open arf history database: {}", path.display()))?;

    // Query all history items
    let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    let items = source
        .search(query)
        .with_context(|| "Failed to query arf history")?;

    let entries: Vec<ImportEntry> = items
        .into_iter()
        .map(|item| ImportEntry {
            command: item.command_line,
            timestamp: item.start_timestamp,
            mode: mode.clone(),
        })
        .collect();

    Ok(entries)
}

/// Target databases for import.
pub struct ImportTargets {
    /// R history database.
    pub r_history: SqliteBackedHistory,
    /// Shell history database.
    pub shell_history: SqliteBackedHistory,
}

/// Determine the target database for an entry based on its mode.
///
/// Returns `Some(true)` for shell, `Some(false)` for R/browse, `None` for unknown modes.
fn classify_mode(mode: Option<&str>) -> Option<bool> {
    match mode {
        Some("shell") => Some(true),               // shell database
        Some("r") | Some("browse") => Some(false), // R database
        None => Some(false),                       // Default to R database
        Some(_) => None,                           // Unknown mode - skip
    }
}

/// Pre-loaded set of existing history entries for duplicate detection (anti-join).
///
/// For entries with timestamps, duplicates are detected by `(command_line, timestamp)`.
/// For entries without timestamps, duplicates are detected by `command_line` alone.
///
/// Note: `commands` intentionally contains **all** command_lines from the database,
/// including those that also have timestamps. This is because a no-timestamp import
/// entry (e.g., from `.Rhistory`) should be considered a duplicate if the same command
/// text already exists in the DB with any timestamp (e.g., from a prior radian import).
/// The `.Rhistory` import is typically a one-time migration, so this conservative
/// approach is acceptable.
pub struct DedupSet {
    /// `(command_line, unix_timestamp_millis)` pairs for matching entries with timestamps.
    command_timestamps: HashSet<(String, i64)>,
    /// All distinct `command_line` values for matching entries without timestamps.
    commands: HashSet<String>,
}

impl DedupSet {
    /// Build a dedup set from an existing history database opened for writing.
    ///
    /// Used in the non-dry-run import path where the database is already opened
    /// via `SqliteBackedHistory::with_file()` for writing.
    ///
    /// Note: reedline's deserialization falls back to `Utc::now()` when a
    /// stored timestamp is not a valid millisecond value. This means the
    /// millis round-trip (`DateTime → i64 → DateTime → i64`) could
    /// theoretically differ from the raw DB value for corrupt rows. In
    /// practice this cannot happen because reedline always writes
    /// `timestamp_millis()`, but [`from_db`] avoids this entirely by
    /// reading the raw i64 directly.
    pub fn from_history(history: &SqliteBackedHistory) -> Result<Self> {
        use reedline::History;

        let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let items = history
            .search(query)
            .context("Failed to query existing history for dedup")?;

        let mut command_timestamps = HashSet::new();
        let mut commands = HashSet::new();

        // INVARIANT: `commands` must contain every command_line that appears
        // in `command_timestamps`, because `is_duplicate` uses `commands` as
        // a fast-path filter for both timestamped and non-timestamped lookups.
        for item in items {
            commands.insert(item.command_line.clone());
            if let Some(ts) = item.start_timestamp {
                command_timestamps.insert((item.command_line, ts.timestamp_millis()));
            }
        }

        Ok(DedupSet {
            command_timestamps,
            commands,
        })
    }

    /// Build a dedup set by opening a history database in read-only mode.
    ///
    /// Used in the dry-run path to avoid WAL/shm side-effect files that
    /// `SqliteBackedHistory::with_file()` would create.
    pub fn from_db(path: &Path) -> Result<Self> {
        use rusqlite::{Connection, OpenFlags};

        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Failed to open history database: {}", path.display()))?;

        // reedline stores start_timestamp as Unix milliseconds (i64) in SQLite.
        // We read the raw value directly to stay consistent with from_history(),
        // which converts DateTime<Utc> back to millis via timestamp_millis().
        let mut stmt = db
            .prepare("SELECT command_line, start_timestamp FROM history")
            .with_context(|| {
                format!(
                    "Failed to query history table in '{}' (not an arf database?)",
                    path.display()
                )
            })?;

        let mut command_timestamps = HashSet::new();
        let mut commands = HashSet::new();

        // INVARIANT: `commands` must contain every command_line that appears
        // in `command_timestamps`, because `is_duplicate` uses `commands` as
        // a fast-path filter for both timestamped and non-timestamped lookups.
        let rows = stmt
            .query_map([], |row| {
                let command: String = row.get(0)?;
                let ts_millis: Option<i64> = row.get(1)?;
                Ok((command, ts_millis))
            })
            .context("Failed to query history for dedup")?;

        for row in rows {
            let (command, ts_millis) = row.context("Failed to read history row")?;
            commands.insert(command.clone());
            if let Some(ms) = ts_millis {
                command_timestamps.insert((command, ms));
            }
        }

        Ok(DedupSet {
            command_timestamps,
            commands,
        })
    }

    /// Check if an entry already exists in the set.
    fn is_duplicate(&self, command: &str, timestamp: Option<&DateTime<Utc>>) -> bool {
        // Fast path: if the command doesn't exist at all, skip the allocation
        // needed for the (String, i64) HashSet lookup.
        if !self.commands.contains(command) {
            return false;
        }
        if let Some(ts) = timestamp {
            self.command_timestamps
                .contains(&(command.to_string(), ts.timestamp_millis()))
        } else {
            true // command exists in commands set (checked above)
        }
    }
}

/// Simulate importing entries without accessing databases.
///
/// Uses the same classification logic as `import_entries` to provide
/// accurate counts and warnings for `--dry-run` mode.
///
/// If dedup sets are provided, duplicate entries will be counted in
/// `duplicates_skipped` instead of being "imported". Each dedup set
/// is optional independently, so dedup works even if only one target
/// database exists.
pub fn import_entries_dry_run(
    entries: &[ImportEntry],
    r_dedup: Option<&DedupSet>,
    shell_dedup: Option<&DedupSet>,
) -> ImportResult {
    let mut result = ImportResult::default();

    for entry in entries {
        if entry.command.trim().is_empty() {
            result.skipped += 1;
            continue;
        }

        // Classify mode and skip unknown modes
        let is_shell = match classify_mode(entry.mode.as_deref()) {
            Some(is_shell) => is_shell,
            None => {
                let mode = entry.mode.as_deref().unwrap_or("?");
                let cmd_preview: String = entry.command.chars().take(30).collect();
                result.warnings.push(format!(
                    "Skipped unknown mode '{}': {}...",
                    mode, cmd_preview
                ));
                result.skipped += 1;
                continue;
            }
        };

        // Check for duplicates if the corresponding dedup set is available
        let dedup_set = if is_shell { shell_dedup } else { r_dedup };
        if let Some(dedup) = dedup_set
            && dedup.is_duplicate(&entry.command, entry.timestamp.as_ref())
        {
            result.duplicates_skipped += 1;
            continue;
        }

        if is_shell {
            result.shell_imported += 1;
        } else {
            result.r_imported += 1;
        }
    }

    result
}

/// Import entries into arf history databases, routing by mode.
///
/// - Entries with mode "shell" go to the shell history database
/// - Entries with mode "r", "browse", or None go to the R history database
/// - Entries with unknown modes are skipped with a warning
///
/// If `hostname_override` is provided, all imported entries will have their
/// hostname field set to this value, making them distinguishable from native
/// arf entries.
///
/// If `skip_duplicates` is true, entries that already exist in the target
/// database are skipped (anti-join on command + timestamp).
///
/// Note: The dedup set is built once from the database state at the start
/// of the import. Duplicates *within* the import batch are not detected
/// (e.g., if the source file contains the same entry twice, both will be
/// imported). This is acceptable because real-world history files rarely
/// contain exact duplicates, and the primary use case is idempotent
/// re-import across separate invocations.
///
/// For dry-run previews, use [`import_entries_dry_run`] instead.
pub fn import_entries(
    targets: &mut ImportTargets,
    entries: Vec<ImportEntry>,
    hostname_override: Option<&str>,
    skip_duplicates: bool,
) -> Result<ImportResult> {
    use reedline::History;

    // Build dedup sets if duplicate skipping is enabled
    let (r_dedup, shell_dedup) = if skip_duplicates {
        (
            Some(DedupSet::from_history(&targets.r_history)?),
            Some(DedupSet::from_history(&targets.shell_history)?),
        )
    } else {
        (None, None)
    };

    let mut result = ImportResult::default();

    for entry in entries {
        if entry.command.trim().is_empty() {
            result.skipped += 1;
            continue;
        }

        // Classify mode and skip unknown modes
        let is_shell = match classify_mode(entry.mode.as_deref()) {
            Some(is_shell) => is_shell,
            None => {
                let mode = entry.mode.as_deref().unwrap_or("?");
                let cmd_preview: String = entry.command.chars().take(30).collect();
                result.warnings.push(format!(
                    "Skipped unknown mode '{}': {}...",
                    mode, cmd_preview
                ));
                result.skipped += 1;
                continue;
            }
        };

        // Check for duplicates if enabled
        if let Some(dedup_set) = if is_shell { &shell_dedup } else { &r_dedup }
            && dedup_set.is_duplicate(&entry.command, entry.timestamp.as_ref())
        {
            result.duplicates_skipped += 1;
            continue;
        }

        // Create a HistoryItem for import
        let item = HistoryItem {
            id: None, // Will be assigned by the database
            command_line: entry.command,
            start_timestamp: entry.timestamp,
            session_id: None,
            hostname: hostname_override.map(|s| s.to_string()),
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: None,
        };

        // Route to appropriate database based on mode
        let save_result = if is_shell {
            targets.shell_history.save(item)
        } else {
            targets.r_history.save(item)
        };

        match save_result {
            Ok(_) => {
                if is_shell {
                    result.shell_imported += 1;
                } else {
                    result.r_imported += 1;
                }
            }
            Err(e) => {
                result
                    .warnings
                    .push(format!("Failed to import entry: {}", e));
                result.skipped += 1;
            }
        }
    }

    Ok(result)
}

/// Validate that a table name is safe for use in SQL queries.
///
/// Table names must contain only alphanumeric characters and underscores,
/// and must not be empty. This prevents SQL injection attacks.
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
    Ok(())
}

/// Parse entries from a unified arf export file that contains both R and shell history.
///
/// This function reads from a SQLite file that has separate tables for R and shell history,
/// as created by `export_history`. The table names are specified by the caller.
///
/// If a table doesn't exist, it's silently skipped (no error).
pub fn parse_unified_arf_history(
    path: &Path,
    r_table: &str,
    shell_table: &str,
) -> Result<Vec<ImportEntry>> {
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

    let mut entries = Vec::new();

    // Try to read R history table
    if table_exists(&db, r_table)? {
        let r_entries = read_history_table(&db, r_table, "r")?;
        entries.extend(r_entries);
    }

    // Try to read shell history table
    if table_exists(&db, shell_table)? {
        let shell_entries = read_history_table(&db, shell_table, "shell")?;
        entries.extend(shell_entries);
    }

    Ok(entries)
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
    table_name: &str,
    mode: &str,
) -> Result<Vec<ImportEntry>> {
    use chrono::TimeZone;

    // Use format! for table name since it can't be parameterized in SQL.
    // Table names are validated by validate_table_name() before reaching here.
    let query = format!(
        "SELECT command_line, start_timestamp FROM \"{}\" ORDER BY id",
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
            let command: String = row.get(0)?;
            let ts_millis: Option<i64> = row.get(1)?;
            Ok((command, ts_millis))
        })
        .context("Failed to query history")?;

    let mut entries = Vec::new();
    for row in rows {
        let (command, ts_millis) = row.context("Failed to read history row")?;
        let timestamp = ts_millis.and_then(|ms| Utc.timestamp_millis_opt(ms).single());
        entries.push(ImportEntry {
            command,
            timestamp,
            mode: Some(mode.to_string()),
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_radian_history_basic() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
        writeln!(file, "# mode: r").unwrap();
        writeln!(file, "+library(dplyr)").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
        writeln!(file, "# mode: shell").unwrap();
        writeln!(file, "+ls -la").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].command, "library(dplyr)");
        assert_eq!(entries[0].mode, Some("r".to_string()));
        assert!(entries[0].timestamp.is_some());

        assert_eq!(entries[1].command, "ls -la");
        assert_eq!(entries[1].mode, Some("shell".to_string()));
    }

    #[test]
    fn test_parse_radian_history_multiline() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
        writeln!(file, "# mode: r").unwrap();
        writeln!(file, "+iris %>%").unwrap();
        writeln!(file, "+  filter(Species == \"setosa\") %>%").unwrap();
        writeln!(file, "+  head()").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].command,
            "iris %>%\n  filter(Species == \"setosa\") %>%\n  head()"
        );
    }

    #[test]
    fn test_parse_radian_history_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let entries = parse_radian_history(file.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_r_history_basic() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "library(dplyr)").unwrap();
        writeln!(file, "print(\"hello\")").unwrap();
        writeln!(file).unwrap(); // Empty line should be skipped
        writeln!(file, "summary(iris)").unwrap();

        let entries = parse_r_history(file.path()).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].command, "library(dplyr)");
        assert_eq!(entries[1].command, "print(\"hello\")");
        assert_eq!(entries[2].command, "summary(iris)");

        // All entries should have mode "r" and no timestamp
        for entry in &entries {
            assert_eq!(entry.mode, Some("r".to_string()));
            assert!(entry.timestamp.is_none());
        }
    }

    #[test]
    fn test_parse_r_history_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let entries = parse_r_history(file.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_r_history_preserves_leading_whitespace() {
        let mut file = NamedTempFile::new().unwrap();
        // Simulate indented code that might appear in .Rhistory
        writeln!(file, "if (TRUE) {{").unwrap();
        writeln!(file, r#"  print("indented")"#).unwrap();
        writeln!(file, "}}").unwrap();

        let entries = parse_r_history(file.path()).unwrap();
        assert_eq!(entries.len(), 3);
        // Leading whitespace should be preserved
        assert_eq!(entries[1].command, r#"  print("indented")"#);
    }

    #[test]
    fn test_default_paths() {
        // These just verify the functions don't panic
        let radian_path = default_radian_path();
        assert!(radian_path.to_string_lossy().contains("radian_history"));

        let r_path = default_r_history_path();
        assert!(
            r_path.to_string_lossy().contains("Rhistory") || std::env::var("R_HISTFILE").is_ok()
        );
    }

    #[test]
    fn test_import_entry_struct() {
        let entry = ImportEntry {
            command: "test".to_string(),
            timestamp: Some(Utc::now()),
            mode: Some("r".to_string()),
        };
        assert_eq!(entry.command, "test");
        assert!(entry.timestamp.is_some());
        assert_eq!(entry.mode, Some("r".to_string()));
    }

    #[test]
    fn test_parse_radian_history_timestamp_parsing() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# time: 2024-06-15 14:30:45 UTC").unwrap();
        writeln!(file, "# mode: r").unwrap();
        writeln!(file, "+test()").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        assert_eq!(entries.len(), 1);

        let ts = entries[0].timestamp.unwrap();
        assert_eq!(
            ts.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-06-15 14:30:45"
        );
    }

    #[test]
    fn test_parse_radian_history_browse_mode() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
        writeln!(file, "# mode: browse").unwrap();
        writeln!(file, "+n").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mode, Some("browse".to_string()));
    }

    fn create_test_targets(temp_dir: &tempfile::TempDir) -> ImportTargets {
        let r_path = temp_dir.path().join("r.db");
        let shell_path = temp_dir.path().join("shell.db");
        ImportTargets {
            r_history: SqliteBackedHistory::with_file(r_path, None, None).unwrap(),
            shell_history: SqliteBackedHistory::with_file(shell_path, None, None).unwrap(),
        }
    }

    #[test]
    fn test_import_entries_to_sqlite() {
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        // Create test entries (R mode)
        let entries = vec![
            ImportEntry {
                command: "library(ggplot2)".to_string(),
                timestamp: Some(Utc::now()),
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "print('hello')".to_string(),
                timestamp: None,
                mode: Some("r".to_string()),
            },
        ];

        let result = import_entries(&mut targets, entries, None, false).unwrap();

        assert_eq!(result.r_imported, 2);
        assert_eq!(result.shell_imported, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.warnings.is_empty());

        // Verify entries were imported to R history
        let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let items = targets.r_history.search(query).unwrap();
        assert_eq!(items.len(), 2);

        let commands: Vec<&str> = items.iter().map(|i| i.command_line.as_str()).collect();
        assert!(commands.contains(&"library(ggplot2)"));
        assert!(commands.contains(&"print('hello')"));
    }

    #[test]
    fn test_import_entries_routes_shell_mode() {
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        // Create mixed mode entries
        let entries = vec![
            ImportEntry {
                command: "library(dplyr)".to_string(),
                timestamp: None,
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "ls -la".to_string(),
                timestamp: None,
                mode: Some("shell".to_string()),
            },
            ImportEntry {
                command: "pwd".to_string(),
                timestamp: None,
                mode: Some("shell".to_string()),
            },
        ];

        let result = import_entries(&mut targets, entries, None, false).unwrap();

        assert_eq!(result.r_imported, 1);
        assert_eq!(result.shell_imported, 2);
        assert_eq!(result.skipped, 0);

        // Verify R history
        let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let r_items = targets.r_history.search(r_query).unwrap();
        assert_eq!(r_items.len(), 1);
        assert_eq!(r_items[0].command_line, "library(dplyr)");

        // Verify shell history
        let shell_query =
            reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let shell_items = targets.shell_history.search(shell_query).unwrap();
        assert_eq!(shell_items.len(), 2);
        let shell_commands: Vec<&str> = shell_items
            .iter()
            .map(|i| i.command_line.as_str())
            .collect();
        assert!(shell_commands.contains(&"ls -la"));
        assert!(shell_commands.contains(&"pwd"));
    }

    #[test]
    fn test_import_entries_dry_run() {
        // Create mixed mode entries
        let entries = vec![
            ImportEntry {
                command: "test_r".to_string(),
                timestamp: None,
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "test_shell".to_string(),
                timestamp: None,
                mode: Some("shell".to_string()),
            },
            ImportEntry {
                command: "unknown_mode".to_string(),
                timestamp: None,
                mode: Some("python".to_string()), // Unknown mode
            },
            ImportEntry {
                command: "   ".to_string(), // Whitespace-only, should be skipped
                timestamp: None,
                mode: Some("r".to_string()),
            },
        ];

        // import_entries_dry_run doesn't need database handles
        let result = import_entries_dry_run(&entries, None, None);

        assert_eq!(result.r_imported, 1);
        assert_eq!(result.shell_imported, 1);
        assert_eq!(result.skipped, 2); // unknown mode + whitespace-only
        assert_eq!(result.warnings.len(), 1); // warning for unknown mode
        assert!(result.warnings[0].contains("python"));
    }

    #[test]
    fn test_import_entries_skips_empty() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        let entries = vec![
            ImportEntry {
                command: "valid".to_string(),
                timestamp: None,
                mode: None,
            },
            ImportEntry {
                command: "   ".to_string(), // Whitespace only - should be skipped
                timestamp: None,
                mode: None,
            },
            ImportEntry {
                command: "".to_string(), // Empty - should be skipped
                timestamp: None,
                mode: None,
            },
        ];

        let result = import_entries(&mut targets, entries, None, false).unwrap();

        assert_eq!(result.r_imported, 1); // "valid" goes to R (mode: None)
        assert_eq!(result.shell_imported, 0);
        assert_eq!(result.skipped, 2);
    }

    #[test]
    fn test_import_entries_skips_unknown_modes() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        let entries = vec![
            ImportEntry {
                command: "valid_r".to_string(),
                timestamp: None,
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "valid_shell".to_string(),
                timestamp: None,
                mode: Some("shell".to_string()),
            },
            ImportEntry {
                command: "unknown_mode_cmd".to_string(),
                timestamp: None,
                mode: Some("python".to_string()), // Unknown mode
            },
            ImportEntry {
                command: "another_unknown".to_string(),
                timestamp: None,
                mode: Some("jupyter".to_string()), // Unknown mode
            },
        ];

        let result = import_entries(&mut targets, entries, None, false).unwrap();

        assert_eq!(result.r_imported, 1);
        assert_eq!(result.shell_imported, 1);
        assert_eq!(result.skipped, 2);
        assert_eq!(result.warnings.len(), 2);
        assert!(result.warnings[0].contains("python"));
        assert!(result.warnings[1].contains("jupyter"));
    }

    #[test]
    fn test_import_entries_handles_browse_mode() {
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        let entries = vec![
            ImportEntry {
                command: "n".to_string(), // debug step
                timestamp: None,
                mode: Some("browse".to_string()),
            },
            ImportEntry {
                command: "c".to_string(), // continue
                timestamp: None,
                mode: Some("browse".to_string()),
            },
        ];

        let result = import_entries(&mut targets, entries, None, false).unwrap();

        // browse mode should go to R database
        assert_eq!(result.r_imported, 2);
        assert_eq!(result.shell_imported, 0);
        assert_eq!(result.skipped, 0);

        // Verify entries are in R history
        let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let items = targets.r_history.search(query).unwrap();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_parse_arf_history_not_found() {
        use tempfile::TempDir;

        // Use TempDir to guarantee a non-existent file path
        let temp_dir = TempDir::new().unwrap();
        let missing_path = temp_dir.path().join("nonexistent.db");

        let result = parse_arf_history(&missing_path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_parse_arf_history_infers_mode_from_filename() {
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();

        // Create an R history database
        let r_path = temp_dir.path().join("r.db");
        let mut r_db = SqliteBackedHistory::with_file(r_path.clone(), None, None).unwrap();
        r_db.save(HistoryItem {
            id: None,
            command_line: "summary(iris)".to_string(),
            start_timestamp: None,
            session_id: None,
            hostname: None,
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: None,
        })
        .unwrap();
        drop(r_db); // Close the database

        // Create a shell history database
        let shell_path = temp_dir.path().join("shell.db");
        let mut shell_db = SqliteBackedHistory::with_file(shell_path.clone(), None, None).unwrap();
        shell_db
            .save(HistoryItem {
                id: None,
                command_line: "ls -la".to_string(),
                start_timestamp: None,
                session_id: None,
                hostname: None,
                cwd: None,
                duration: None,
                exit_status: None,
                more_info: None,
            })
            .unwrap();
        drop(shell_db);

        // Parse R history - should have mode "r"
        let r_entries = parse_arf_history(&r_path).unwrap();
        assert_eq!(r_entries.len(), 1);
        assert_eq!(r_entries[0].mode, Some("r".to_string()));

        // Parse shell history - should have mode "shell"
        let shell_entries = parse_arf_history(&shell_path).unwrap();
        assert_eq!(shell_entries.len(), 1);
        assert_eq!(shell_entries[0].mode, Some("shell".to_string()));
    }

    #[test]
    fn test_arf_shell_to_shell_import() {
        use reedline::History;
        use tempfile::TempDir;

        // Use separate directories for source and target to avoid conflicts
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        // Create a source shell history database
        let source_path = source_dir.path().join("old_shell.db");
        let mut source_db =
            SqliteBackedHistory::with_file(source_path.clone(), None, None).unwrap();
        source_db
            .save(HistoryItem {
                id: None,
                command_line: "git status".to_string(),
                start_timestamp: Some(Utc::now()),
                session_id: None,
                hostname: None,
                cwd: None,
                duration: None,
                exit_status: None,
                more_info: None,
            })
            .unwrap();
        drop(source_db);

        // Note: filename doesn't end with "shell.db" so it will be treated as R
        // This tests that only exact "shell.db" filename triggers shell mode
        let entries = parse_arf_history(&source_path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mode, Some("r".to_string())); // Not shell!

        // Now test with exact "shell.db" filename
        let shell_source_path = source_dir.path().join("shell.db");
        std::fs::copy(&source_path, &shell_source_path).unwrap();

        let shell_entries = parse_arf_history(&shell_source_path).unwrap();
        assert_eq!(shell_entries.len(), 1);
        assert_eq!(shell_entries[0].mode, Some("shell".to_string()));

        // Import to target databases (in separate directory)
        let mut targets = create_test_targets(&target_dir);
        let result = import_entries(&mut targets, shell_entries, None, false).unwrap();

        assert_eq!(result.r_imported, 0);
        assert_eq!(result.shell_imported, 1);

        // Verify it went to shell database
        let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let items = targets.shell_history.search(query).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].command_line, "git status");
    }

    #[test]
    fn test_end_to_end_radian_import_with_shell() {
        use reedline::History;
        use tempfile::TempDir;

        // Create a radian history file with both R and shell commands
        let mut source_file = NamedTempFile::new().unwrap();
        writeln!(source_file, "# time: 2024-03-15 09:00:00 UTC").unwrap();
        writeln!(source_file, "# mode: r").unwrap();
        writeln!(source_file, "+summary(mtcars)").unwrap();
        writeln!(source_file).unwrap();
        writeln!(source_file, "# time: 2024-03-15 09:01:00 UTC").unwrap();
        writeln!(source_file, "# mode: shell").unwrap();
        writeln!(source_file, "+git status").unwrap();
        writeln!(source_file).unwrap();
        writeln!(source_file, "# time: 2024-03-15 09:02:00 UTC").unwrap();
        writeln!(source_file, "# mode: r").unwrap();
        writeln!(source_file, "+plot(mtcars$mpg, mtcars$hp)").unwrap();

        // Parse the radian history
        let entries = parse_radian_history(source_file.path()).unwrap();
        assert_eq!(entries.len(), 3);

        // Import to SQLite
        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        let result = import_entries(&mut targets, entries, None, false).unwrap();
        assert_eq!(result.r_imported, 2);
        assert_eq!(result.shell_imported, 1);

        // Verify R history
        let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let r_items = targets.r_history.search(r_query).unwrap();
        assert_eq!(r_items.len(), 2);
        // Check timestamps were preserved
        assert!(r_items.iter().all(|i| i.start_timestamp.is_some()));

        // Verify shell history
        let shell_query =
            reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let shell_items = targets.shell_history.search(shell_query).unwrap();
        assert_eq!(shell_items.len(), 1);
        assert_eq!(shell_items[0].command_line, "git status");
        assert!(shell_items[0].start_timestamp.is_some());
    }

    #[test]
    fn test_import_entries_with_hostname_override() {
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        let entries = vec![
            ImportEntry {
                command: "library(dplyr)".to_string(),
                timestamp: None,
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "ls -la".to_string(),
                timestamp: None,
                mode: Some("shell".to_string()),
            },
        ];

        // Import with custom hostname
        let result = import_entries(&mut targets, entries, Some("radian-import"), false).unwrap();

        assert_eq!(result.r_imported, 1);
        assert_eq!(result.shell_imported, 1);

        // Verify R history has the custom hostname
        let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let r_items = targets.r_history.search(r_query).unwrap();
        assert_eq!(r_items.len(), 1);
        assert_eq!(r_items[0].hostname, Some("radian-import".to_string()));

        // Verify shell history also has the custom hostname
        let shell_query =
            reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let shell_items = targets.shell_history.search(shell_query).unwrap();
        assert_eq!(shell_items.len(), 1);
        assert_eq!(shell_items[0].hostname, Some("radian-import".to_string()));
    }

    // === Edge case tests for regression prevention ===

    #[test]
    fn test_parse_radian_history_mode_not_carried_over() {
        // Regression test: mode should NOT carry over from previous entry
        // when a new timestamp boundary is encountered without a mode line.
        let mut file = NamedTempFile::new().unwrap();
        // First entry with explicit shell mode
        writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
        writeln!(file, "# mode: shell").unwrap();
        writeln!(file, "+ls -la").unwrap();
        writeln!(file).unwrap();
        // Second entry WITHOUT mode line - should NOT inherit "shell" from previous
        writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
        writeln!(file, "+library(dplyr)").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        assert_eq!(entries.len(), 2);

        // First entry should be shell
        assert_eq!(entries[0].command, "ls -la");
        assert_eq!(entries[0].mode, Some("shell".to_string()));

        // Second entry should have no mode (None), not "shell"
        assert_eq!(entries[1].command, "library(dplyr)");
        assert_eq!(entries[1].mode, None);
    }

    #[test]
    fn test_parse_radian_history_consecutive_timestamps_without_commands() {
        // Edge case: consecutive timestamp headers without commands should not cause issues
        let mut file = NamedTempFile::new().unwrap();
        // First timestamp with no command lines
        writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
        writeln!(file, "# mode: r").unwrap();
        // Empty line acts as separator
        writeln!(file).unwrap();
        // Second timestamp immediately follows
        writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
        writeln!(file, "# mode: shell").unwrap();
        writeln!(file, "+git status").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        // Only one entry should be parsed (the one with actual command)
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "git status");
        assert_eq!(entries[0].mode, Some("shell".to_string()));
    }

    #[test]
    fn test_parse_radian_history_file_not_found() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let missing_path = temp_dir.path().join("nonexistent_radian_history");

        let result = parse_radian_history(&missing_path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to open radian history"));
    }

    #[test]
    fn test_parse_r_history_file_not_found() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let missing_path = temp_dir.path().join("nonexistent_Rhistory");

        let result = parse_r_history(&missing_path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to open R history"));
    }

    #[test]
    fn test_parse_radian_history_mode_reset_between_entries() {
        // Another regression test: ensure mode is properly reset between entries
        // even when separated by empty lines
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
        writeln!(file, "# mode: shell").unwrap();
        writeln!(file, "+pwd").unwrap();
        writeln!(file).unwrap(); // Empty line separator
        writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
        // No mode line for this entry
        writeln!(file, "+summary(iris)").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "# time: 2024-01-15 10:32:00 UTC").unwrap();
        writeln!(file, "# mode: browse").unwrap();
        writeln!(file, "+n").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].mode, Some("shell".to_string()));
        assert_eq!(entries[1].mode, None); // Mode was reset, not carried over
        assert_eq!(entries[2].mode, Some("browse".to_string()));
    }

    #[test]
    fn test_parse_r_history_whitespace_only_lines_skipped() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "library(dplyr)").unwrap();
        writeln!(file, "   ").unwrap(); // Whitespace-only line
        writeln!(file, "\t").unwrap(); // Tab-only line
        writeln!(file, "print(1)").unwrap();

        let entries = parse_r_history(file.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].command, "library(dplyr)");
        assert_eq!(entries[1].command, "print(1)");
    }

    #[test]
    fn test_import_entries_none_mode_goes_to_r_database() {
        // Entries with mode=None should go to R database (default behavior)
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        let entries = vec![ImportEntry {
            command: "summary(mtcars)".to_string(),
            timestamp: None,
            mode: None, // No mode specified
        }];

        let result = import_entries(&mut targets, entries, None, false).unwrap();
        assert_eq!(result.r_imported, 1);
        assert_eq!(result.shell_imported, 0);

        // Verify it's in R history
        let query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let items = targets.r_history.search(query).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].command_line, "summary(mtcars)");
    }

    #[test]
    fn test_parse_radian_history_crlf_line_endings() {
        // Test that CRLF line endings (Windows) are handled correctly
        let mut file = NamedTempFile::new().unwrap();
        // Write with explicit \r\n
        file.write_all(b"# time: 2024-01-15 10:30:00 UTC\r\n")
            .unwrap();
        file.write_all(b"# mode: r\r\n").unwrap();
        file.write_all(b"+print(1)\r\n").unwrap();

        let entries = parse_radian_history(file.path()).unwrap();
        assert_eq!(entries.len(), 1);
        // Command should not have trailing \r
        assert_eq!(entries[0].command, "print(1)");
        assert!(!entries[0].command.ends_with('\r'));
    }

    // === Dedup (anti-join) tests ===

    #[test]
    fn test_import_skips_duplicates_with_timestamp() {
        use reedline::History;
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
        }];
        let result = import_entries(&mut targets, entries, None, true).unwrap();
        assert_eq!(result.r_imported, 1);
        assert_eq!(result.duplicates_skipped, 0);

        // Second import of the same entry: should be skipped as duplicate
        let entries = vec![ImportEntry {
            command: "library(dplyr)".to_string(),
            timestamp: Some(ts),
            mode: Some("r".to_string()),
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
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        // First import (no timestamp)
        let entries = vec![ImportEntry {
            command: "summary(iris)".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
        }];
        let result = import_entries(&mut targets, entries, None, true).unwrap();
        assert_eq!(result.r_imported, 1);
        assert_eq!(result.duplicates_skipped, 0);

        // Second import of the same command (no timestamp): should be skipped
        let entries = vec![ImportEntry {
            command: "summary(iris)".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
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
        use reedline::History;
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
        }];
        let result = import_entries(&mut targets, entries, None, true).unwrap();
        assert_eq!(result.r_imported, 1);

        let entries = vec![ImportEntry {
            command: "library(dplyr)".to_string(),
            timestamp: Some(ts2),
            mode: Some("r".to_string()),
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
        use reedline::History;
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
        }];
        let result = import_entries(&mut targets, entries, None, false).unwrap();
        assert_eq!(result.r_imported, 1);

        // Second import with skip_duplicates=false (--import-duplicates)
        let entries = vec![ImportEntry {
            command: "library(dplyr)".to_string(),
            timestamp: Some(ts),
            mode: Some("r".to_string()),
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
        use reedline::History;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let mut targets = create_test_targets(&temp_dir);

        // Import an R command
        let entries = vec![ImportEntry {
            command: "ls -la".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
        }];
        let result = import_entries(&mut targets, entries, None, true).unwrap();
        assert_eq!(result.r_imported, 1);

        // Import the same command as shell — should NOT be a duplicate
        // because it's checked against the shell database, not R
        let entries = vec![ImportEntry {
            command: "ls -la".to_string(),
            timestamp: None,
            mode: Some("shell".to_string()),
        }];
        let result = import_entries(&mut targets, entries, None, true).unwrap();
        assert_eq!(result.shell_imported, 1);
        assert_eq!(result.duplicates_skipped, 0);

        // Verify both databases have the entry
        let r_query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
        let r_items = targets.r_history.search(r_query).unwrap();
        assert_eq!(r_items.len(), 1);

        let shell_query =
            reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
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
            },
            ImportEntry {
                command: "print(1)".to_string(), // new
                timestamp: None,
                mode: Some("r".to_string()),
            },
        ];

        let result = import_entries_dry_run(&entries, Some(&r_dedup), Some(&shell_dedup));
        assert_eq!(result.r_imported, 1);
        assert_eq!(result.duplicates_skipped, 1);
    }

    #[test]
    fn test_import_mixed_dedup_new_and_existing() {
        use reedline::History;
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
        }];
        import_entries(&mut targets, entries, None, false).unwrap();

        // Import a batch with duplicates and new entries
        let entries = vec![
            ImportEntry {
                command: "library(dplyr)".to_string(), // duplicate
                timestamp: Some(ts),
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "print(1)".to_string(), // new
                timestamp: None,
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "git status".to_string(), // new (shell)
                timestamp: None,
                mode: Some("shell".to_string()),
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

        let shell_query =
            reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
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
        }];
        import_entries(&mut targets, entries, None, false).unwrap();

        // Build dedup set only for R (simulating shell.db not existing)
        let r_dedup = DedupSet::from_history(&targets.r_history).unwrap();

        let entries = vec![
            ImportEntry {
                command: "library(dplyr)".to_string(), // duplicate in R
                timestamp: None,
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "print(1)".to_string(), // new R entry
                timestamp: None,
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "ls -la".to_string(), // shell entry, no dedup set
                timestamp: None,
                mode: Some("shell".to_string()),
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
        use reedline::History;
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
        }];
        let result = import_entries(&mut targets, entries, None, true).unwrap();
        assert_eq!(result.r_imported, 1);

        // Try to import the same command without a timestamp: should be skipped
        let entries = vec![ImportEntry {
            command: "library(dplyr)".to_string(),
            timestamp: None,
            mode: Some("r".to_string()),
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
            },
            ImportEntry {
                command: "print(1)".to_string(),
                timestamp: Some(ts2),
                mode: Some("r".to_string()),
            },
            ImportEntry {
                command: "summary(iris)".to_string(),
                timestamp: None, // no timestamp
                mode: Some("r".to_string()),
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

        db.execute("INSERT INTO r (command_line, start_timestamp) VALUES ('library(dplyr)', 1705315800000)", []).unwrap();
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
}
