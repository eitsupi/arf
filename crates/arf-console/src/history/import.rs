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
use reedline::{HistoryItem, HistoryItemId, HistorySessionId};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::metadata::HistoryExtraInfo;
use super::store::HistoryStore;

/// The destination selected for an imported item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportMode {
    R,
    Shell,
    Browse,
    Unspecified,
    Unsupported(String),
}

impl ImportMode {
    fn from_external(mode: Option<&str>) -> Self {
        match mode {
            Some("r") => Self::R,
            Some("shell") => Self::Shell,
            Some("browse") => Self::Browse,
            Some(mode) => Self::Unsupported(mode.to_owned()),
            None => Self::Unspecified,
        }
    }
}

/// A parsed history entry ready for import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportEntry {
    pub mode: ImportMode,
    pub item: HistoryItem<HistoryExtraInfo>,
}

impl ImportEntry {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            mode: ImportMode::Unspecified,
            item: HistoryItem {
                id: None,
                start_timestamp: None,
                command_line: command.into(),
                session_id: None,
                hostname: None,
                cwd: None,
                duration: None,
                exit_status: None,
                more_info: None,
            },
        }
    }

    pub fn with_mode(mut self, mode: ImportMode) -> Self {
        self.mode = mode;
        self
    }
}

/// Parsed entries together with non-fatal row warnings.
#[derive(Debug, Default)]
pub struct ParsedImport {
    pub entries: Vec<ImportEntry>,
    pub warnings: Vec<String>,
}

impl std::ops::Deref for ParsedImport {
    type Target = [ImportEntry];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

/// Result of an import operation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportResult {
    /// Number of R entries successfully imported.
    pub r_imported: usize,
    /// Number of shell entries successfully imported.
    pub shell_imported: usize,
    /// Number of entries skipped (empty, unknown mode, errors).
    pub skipped: usize,
    /// Number of duplicate entries skipped.
    pub duplicates_skipped: usize,
    /// Number of existing duplicate rows whose missing fields were repaired.
    pub duplicates_repaired: usize,
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
/// The mode is inferred from the filename:
/// - Files named `shell.db` are treated as shell history
/// - All other files are treated as R history
pub fn parse_arf_history(path: &Path) -> Result<ParsedImport> {
    if !path.exists() {
        bail!("arf history database not found: {}", path.display());
    }

    // Infer mode from filename
    let is_shell = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "shell.db");
    let mode = if is_shell {
        ImportMode::Shell
    } else {
        ImportMode::R
    };

    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Failed to open arf history database: {}", path.display()))?;
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

/// Target databases for import.
pub struct ImportTargets {
    /// R history database.
    pub r_history: HistoryStore,
    /// Shell history database.
    pub shell_history: HistoryStore,
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
#[derive(Clone)]
pub struct DedupSet {
    /// `(command_line, unix_timestamp_millis)` pairs for matching entries with timestamps.
    command_timestamps: HashSet<(String, i64)>,
    /// All distinct `command_line` values for matching entries without timestamps.
    commands: HashSet<String>,
    rows: Vec<DedupRow>,
    command_timestamps_by_row: HashMap<(String, i64), Vec<usize>>,
    command_rows: HashMap<String, Vec<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetadataState {
    Null,
    Valid,
    Malformed,
}

#[derive(Debug, Clone)]
struct DedupRow {
    id: HistoryItemId,
    has_session_id: bool,
    has_hostname: bool,
    has_cwd: bool,
    has_duration: bool,
    has_exit_status: bool,
    metadata: MetadataState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DuplicateAction {
    NotDuplicate,
    Skip,
    Repair(HistoryItemId),
    Ambiguous,
    Malformed,
}

impl DedupSet {
    /// Build a dedup set while the writable target store is already open.
    pub fn from_history(history: &HistoryStore) -> Result<Self> {
        Self::from_connection(rusqlite::Connection::open(history.path()).with_context(|| {
            format!(
                "Failed to read history database: {}",
                history.path().display()
            )
        })?)
    }

    /// Build a dedup set by opening a history database in read-only mode.
    ///
    /// Used in the dry-run path to avoid WAL/shm side-effect files that
    /// `SqliteBackedHistory::with_file()` would create.
    pub fn from_db(path: &Path) -> Result<Self> {
        use rusqlite::{Connection, OpenFlags};

        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Failed to open history database: {}", path.display()))?;
        Self::from_connection(db)
    }

    fn from_connection(db: rusqlite::Connection) -> Result<Self> {
        let columns = HistoryTableColumns::read(&db, "history")?;
        let query = format!(
            "SELECT id, command_line, start_timestamp, {}, {}, {}, {}, {}, {} FROM history",
            columns.expression("session_id"),
            columns.expression("hostname"),
            columns.expression("cwd"),
            columns.expression("duration_ms"),
            columns.expression("exit_status"),
            columns.expression("more_info"),
        );
        let mut stmt = db
            .prepare(&query)
            .context("Failed to query history for dedup")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    HistoryItemId::new(row.get::<_, i64>(0)?),
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?.is_some(),
                    row.get::<_, Option<String>>(4)?.is_some(),
                    row.get::<_, Option<String>>(5)?.is_some(),
                    row.get::<_, Option<i64>>(6)?.is_some(),
                    row.get::<_, Option<i64>>(7)?.is_some(),
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .context("Failed to query history for dedup")?;

        let mut set = Self {
            command_timestamps: HashSet::new(),
            commands: HashSet::new(),
            rows: Vec::new(),
            command_timestamps_by_row: HashMap::new(),
            command_rows: HashMap::new(),
        };
        for row in rows {
            let (
                id,
                command,
                timestamp_millis,
                has_session_id,
                has_hostname,
                has_cwd,
                has_duration,
                has_exit_status,
                raw_metadata,
            ) = row.context("Failed to read history row")?;
            let metadata = match raw_metadata.as_deref() {
                None => MetadataState::Null,
                Some(raw) => match serde_json::from_str::<HistoryExtraInfo>(raw) {
                    Ok(_) => MetadataState::Valid,
                    Err(_) => MetadataState::Malformed,
                },
            };
            let row_index = set.rows.len();
            set.commands.insert(command.clone());
            set.command_rows
                .entry(command.clone())
                .or_default()
                .push(row_index);
            if let Some(ms) = timestamp_millis {
                set.command_timestamps.insert((command.clone(), ms));
                set.command_timestamps_by_row
                    .entry((command.clone(), ms))
                    .or_default()
                    .push(row_index);
            }
            set.rows.push(DedupRow {
                id,
                has_session_id,
                has_hostname,
                has_cwd,
                has_duration,
                has_exit_status,
                metadata,
            });
        }
        Ok(set)
    }

    /// Check if an entry already exists in the set.
    #[allow(dead_code)]
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

    fn duplicate_action(
        &self,
        command: &str,
        timestamp: Option<&DateTime<Utc>>,
        item: &HistoryItem<HistoryExtraInfo>,
    ) -> DuplicateAction {
        let row_indices = if let Some(ts) = timestamp {
            self.command_timestamps_by_row
                .get(&(command.to_string(), ts.timestamp_millis()))
        } else {
            self.command_rows.get(command)
        };
        let Some(row_indices) = row_indices else {
            return DuplicateAction::NotDuplicate;
        };
        if row_indices.len() != 1 {
            return DuplicateAction::Ambiguous;
        }
        let row = &self.rows[row_indices[0]];
        let needs_repair = (item.session_id.is_some() && !row.has_session_id)
            || (item.hostname.is_some() && !row.has_hostname)
            || (item.cwd.is_some() && !row.has_cwd)
            || (item.duration.is_some() && !row.has_duration)
            || (item.exit_status.is_some() && !row.has_exit_status)
            || (item.more_info.is_some() && matches!(row.metadata, MetadataState::Null));
        if needs_repair {
            return DuplicateAction::Repair(row.id);
        }
        if matches!(row.metadata, MetadataState::Malformed) && item.more_info.is_some() {
            DuplicateAction::Malformed
        } else {
            DuplicateAction::Skip
        }
    }

    fn mark_repaired(&mut self, id: HistoryItemId, source: &HistoryItem<HistoryExtraInfo>) {
        let Some(row) = self.rows.iter_mut().find(|row| row.id == id) else {
            return;
        };
        row.has_session_id |= source.session_id.is_some();
        row.has_hostname |= source.hostname.is_some();
        row.has_cwd |= source.cwd.is_some();
        row.has_duration |= source.duration.is_some();
        row.has_exit_status |= source.exit_status.is_some();
        if source.more_info.is_some() && matches!(row.metadata, MetadataState::Null) {
            row.metadata = MetadataState::Valid;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportTarget {
    R,
    Shell,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EntryPlan {
    Insert(ImportTarget),
    Repair {
        target: ImportTarget,
        id: HistoryItemId,
    },
    Duplicate,
    SkipEmpty,
    SkipUnsupported {
        mode: String,
    },
    AmbiguousRepair,
    MalformedExistingMetadata,
}

pub(crate) fn plan_entry(
    entry: &ImportEntry,
    r_dedup: Option<&DedupSet>,
    shell_dedup: Option<&DedupSet>,
) -> EntryPlan {
    if entry.item.command_line.trim().is_empty() {
        return EntryPlan::SkipEmpty;
    }

    let target = match &entry.mode {
        ImportMode::R | ImportMode::Browse | ImportMode::Unspecified => ImportTarget::R,
        ImportMode::Shell => ImportTarget::Shell,
        ImportMode::Unsupported(mode) => {
            return EntryPlan::SkipUnsupported { mode: mode.clone() };
        }
    };
    let dedup = match target {
        ImportTarget::R => r_dedup,
        ImportTarget::Shell => shell_dedup,
    };
    let Some(dedup) = dedup else {
        return EntryPlan::Insert(target);
    };

    match dedup.duplicate_action(
        &entry.item.command_line,
        entry.item.start_timestamp.as_ref(),
        &entry.item,
    ) {
        DuplicateAction::NotDuplicate => EntryPlan::Insert(target),
        DuplicateAction::Skip => EntryPlan::Duplicate,
        DuplicateAction::Repair(id) => EntryPlan::Repair { target, id },
        DuplicateAction::Ambiguous => EntryPlan::AmbiguousRepair,
        DuplicateAction::Malformed => EntryPlan::MalformedExistingMetadata,
    }
}

fn add_plan_warning(result: &mut ImportResult, entry: &ImportEntry, plan: &EntryPlan) {
    let command = &entry.item.command_line;
    match plan {
        EntryPlan::SkipUnsupported { mode } => {
            let preview: String = command.chars().take(30).collect();
            result
                .warnings
                .push(format!("Skipped unknown mode '{}': {}...", mode, preview));
        }
        EntryPlan::AmbiguousRepair => result.warnings.push(format!(
            "Could not repair duplicate command '{}': matches multiple rows",
            command
        )),
        EntryPlan::MalformedExistingMetadata => result.warnings.push(format!(
            "Could not repair duplicate command '{}': existing metadata is malformed; leaving it unchanged",
            command
        )),
        _ => {}
    }
}

fn record_plan(result: &mut ImportResult, entry: &ImportEntry, plan: &EntryPlan) {
    match plan {
        EntryPlan::Insert(ImportTarget::R) => result.r_imported += 1,
        EntryPlan::Insert(ImportTarget::Shell) => result.shell_imported += 1,
        EntryPlan::Repair { .. } => result.duplicates_repaired += 1,
        EntryPlan::Duplicate => result.duplicates_skipped += 1,
        EntryPlan::SkipEmpty => result.skipped += 1,
        EntryPlan::SkipUnsupported { .. }
        | EntryPlan::AmbiguousRepair
        | EntryPlan::MalformedExistingMetadata => {
            result.skipped += usize::from(matches!(plan, EntryPlan::SkipUnsupported { .. }));
            if !matches!(plan, EntryPlan::SkipUnsupported { .. }) {
                result.duplicates_skipped += 1;
            }
        }
    }
    add_plan_warning(result, entry, plan);
}

/// Simulate importing entries without accessing databases.
pub fn import_entries_dry_run(
    entries: &[ImportEntry],
    r_dedup: Option<&DedupSet>,
    shell_dedup: Option<&DedupSet>,
) -> ImportResult {
    let mut r_dedup = r_dedup.cloned();
    let mut shell_dedup = shell_dedup.cloned();
    let mut result = ImportResult::default();
    for entry in entries {
        let plan = plan_entry(entry, r_dedup.as_ref(), shell_dedup.as_ref());
        record_plan(&mut result, entry, &plan);
        if let EntryPlan::Repair { target, id } = plan {
            match target {
                ImportTarget::R => r_dedup
                    .as_mut()
                    .expect("repair requires an R dedup set")
                    .mark_repaired(id, &entry.item),
                ImportTarget::Shell => shell_dedup
                    .as_mut()
                    .expect("repair requires a shell dedup set")
                    .mark_repaired(id, &entry.item),
            }
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
/// of the import. Plain insertion does not add new rows to that snapshot, so
/// duplicates *within* the import batch are still not detected (e.g., if the
/// source file contains the same new entry twice, both will be imported).
/// Successful repairs do advance the missing-field state in the snapshot,
/// because a second repair of the same row would otherwise plan work that the
/// transactional update correctly finds already complete. This preserves the
/// deliberate insertion behavior while keeping repeated repairs consistent
/// with dry-run planning.
///
/// For dry-run previews, use [`import_entries_dry_run`] instead.
pub fn import_entries(
    targets: &mut ImportTargets,
    entries: Vec<ImportEntry>,
    hostname_override: Option<&str>,
    skip_duplicates: bool,
) -> Result<ImportResult> {
    let (mut r_dedup, mut shell_dedup) = if skip_duplicates {
        (
            Some(DedupSet::from_history(&targets.r_history)?),
            Some(DedupSet::from_history(&targets.shell_history)?),
        )
    } else {
        (None, None)
    };

    let mut result = ImportResult::default();

    for mut entry in entries {
        if let Some(hostname) = hostname_override {
            entry.item.hostname = Some(hostname.to_owned());
        }
        let plan = plan_entry(&entry, r_dedup.as_ref(), shell_dedup.as_ref());
        match plan {
            EntryPlan::Insert(target) => {
                let mut item = entry.item;
                item.id = None;
                let save_result = match target {
                    ImportTarget::R => targets.r_history.save_imported(item),
                    ImportTarget::Shell => targets.shell_history.save_imported(item),
                };
                match save_result {
                    Ok(_) => match target {
                        ImportTarget::R => result.r_imported += 1,
                        ImportTarget::Shell => result.shell_imported += 1,
                    },
                    Err(error) => {
                        result
                            .warnings
                            .push(format!("Failed to import entry: {}", error));
                        result.skipped += 1;
                    }
                }
            }
            EntryPlan::Repair { target, id } => {
                let command = entry.item.command_line.clone();
                let store = match target {
                    ImportTarget::R => &targets.r_history,
                    ImportTarget::Shell => &targets.shell_history,
                };
                let source = entry.item;
                match store.set_missing_fields_if_empty(id, source.clone()) {
                    Ok(true) => {
                        match target {
                            ImportTarget::R => r_dedup
                                .as_mut()
                                .expect("repair requires an R dedup set")
                                .mark_repaired(id, &source),
                            ImportTarget::Shell => shell_dedup
                                .as_mut()
                                .expect("repair requires a shell dedup set")
                                .mark_repaired(id, &source),
                        }
                        result.duplicates_repaired += 1
                    }
                    Ok(false) => result.duplicates_skipped += 1,
                    Err(error) => {
                        result.warnings.push(format!(
                            "Failed to repair duplicate '{}': {}",
                            command, error
                        ));
                        result.duplicates_skipped += 1;
                    }
                }
            }
            other => record_plan(&mut result, &entry, &other),
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
        "SELECT id, command_line, start_timestamp, {}, {}, {}, {}, {}, {} FROM \"{}\" ORDER BY id",
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

struct HistoryTableColumns {
    names: HashSet<String>,
}

impl HistoryTableColumns {
    fn read(db: &rusqlite::Connection, table_name: &str) -> Result<Self> {
        let mut names = HashSet::new();
        let query = format!("PRAGMA table_info(\"{}\")", table_name);
        let mut stmt = db
            .prepare(&query)
            .context("Failed to inspect history table")?;
        let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for column in columns {
            names.insert(column?);
        }
        Ok(Self { names })
    }

    fn expression(&self, column: &str) -> String {
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

#[cfg(test)]
mod tests;
