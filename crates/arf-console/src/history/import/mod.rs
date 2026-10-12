//! History import functionality for migrating from other R environments.
//!
//! This module provides importers for:
//! - **radian**: Parse `~/.radian_history` format with timestamps and modes
//! - **R native**: Parse `.Rhistory` plain text format
//! - **arf**: Copy from another arf SQLite database
//!
//! The external-source readers and import execution pipeline are kept in
//! focused submodules while this module preserves the public facade.

use reedline::HistoryItem;

use super::metadata::HistoryExtraInfo;
#[cfg(test)]
use super::store::HistoryStore;

mod execution;
mod source;

#[cfg(test)]
use execution::import_entries_with_dedup_sets;
pub use execution::{DedupSet, ImportTargets, import_entries, import_entries_dry_run};
#[allow(unused_imports)]
pub(crate) use execution::{EntryPlan, ImportTarget, plan_entry};
pub use source::{
    default_r_history_path, default_radian_path, parse_arf_history, parse_r_history,
    parse_radian_history, parse_unified_arf_history, validate_table_name,
};

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

#[cfg(test)]
mod tests;
