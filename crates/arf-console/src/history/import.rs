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
mod tests;
