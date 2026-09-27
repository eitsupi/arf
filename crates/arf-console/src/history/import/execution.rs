//! Deduplication planning and execution for history imports.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reedline::{HistoryItem, HistoryItemId};
use std::collections::HashMap;
use std::path::Path;

use super::super::metadata::HistoryExtraInfo;
use super::super::store::HistoryStore;
use super::source::HistoryTableColumns;
use super::{ImportEntry, ImportMode, ImportResult};

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
#[derive(Clone)]
pub struct DedupSet {
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
        let path = history
            .path()
            .ok_or_else(|| anyhow::anyhow!("history store has no persistent path"))?;
        Self::from_connection(
            rusqlite::Connection::open(path)
                .with_context(|| format!("Failed to read history database: {}", path.display()))?,
        )
    }

    /// Build a dedup set by opening a history database in read-only mode.
    ///
    /// Used in the dry-run path to avoid WAL/shm side-effect files that
    /// `SqliteBackedHistory::with_file()` would create.
    pub(crate) fn from_db(
        path: &Path,
        expected_kind: super::super::artifact::HistoryKind,
    ) -> Result<Self> {
        use rusqlite::{Connection, OpenFlags};

        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Failed to open history database: {}", path.display()))?;
        super::super::artifact::validate_history_artifact(&db, expected_kind)
            .with_context(|| format!("Invalid history database: {}", path.display()))?;
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
            set.command_rows
                .entry(command.clone())
                .or_default()
                .push(row_index);
            if let Some(ms) = timestamp_millis {
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
    let (r_dedup, shell_dedup) = if skip_duplicates {
        (
            Some(DedupSet::from_history(&targets.r_history)?),
            Some(DedupSet::from_history(&targets.shell_history)?),
        )
    } else {
        (None, None)
    };

    import_entries_with_dedup_sets(targets, entries, hostname_override, r_dedup, shell_dedup)
}

pub(super) fn import_entries_with_dedup_sets(
    targets: &mut ImportTargets,
    entries: Vec<ImportEntry>,
    hostname_override: Option<&str>,
    mut r_dedup: Option<DedupSet>,
    mut shell_dedup: Option<DedupSet>,
) -> Result<ImportResult> {
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
