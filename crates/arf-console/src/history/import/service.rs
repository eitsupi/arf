//! Orchestration shared by every command that imports into arf history databases.
//!
//! Source parsing stays with the callers; this module owns what happens once
//! entries exist: previewing against the destination, opening the destination
//! databases, running the import, and reporting the result.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::super::HistoryStore;
use super::super::artifact::HistoryKind;
use super::{
    DedupSet, ImportEntry, ImportResult, ImportTargets, import_entries, import_entries_dry_run,
};

/// Database files inside one history directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryDbPaths {
    pub(crate) directory: PathBuf,
    pub(crate) r: PathBuf,
    pub(crate) shell: PathBuf,
}

impl HistoryDbPaths {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self {
            r: directory.join("r.db"),
            shell: directory.join("shell.db"),
            directory,
        }
    }

    pub(crate) fn path(&self, kind: HistoryKind) -> &Path {
        match kind {
            HistoryKind::R => &self.r,
            HistoryKind::Shell => &self.shell,
        }
    }
}

/// Options that shape how entries are applied to the destination.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImportPolicy<'a> {
    pub(crate) hostname: Option<&'a str>,
    pub(crate) skip_duplicates: bool,
}

/// Return the first kind whose `source` file is the same file as the
/// destination database, which would make an import duplicate its own rows.
pub(crate) fn shared_database(source: &Path, destination: &HistoryDbPaths) -> Option<HistoryKind> {
    let source = fs::canonicalize(source).ok()?;
    [HistoryKind::R, HistoryKind::Shell]
        .into_iter()
        .find(|&kind| fs::canonicalize(destination.path(kind)).is_ok_and(|target| target == source))
}

/// Simulate an import without creating or modifying any destination file.
///
/// Dedup sets are read from whichever destination databases already exist,
/// each checked independently. `destination` is `None` when no persistent
/// history directory could be resolved.
pub(crate) fn preview(
    entries: &[ImportEntry],
    destination: Option<&HistoryDbPaths>,
    policy: ImportPolicy<'_>,
) -> Result<ImportResult> {
    let (r_dedup, shell_dedup) = if policy.skip_duplicates {
        if let Some(destination) = destination {
            let existing = |kind| -> Result<Option<DedupSet>> {
                let path = destination.path(kind);
                path.exists()
                    .then(|| DedupSet::from_db(path, kind))
                    .transpose()
            };
            (existing(HistoryKind::R)?, existing(HistoryKind::Shell)?)
        } else {
            // Volatile mode and similar setups have no directory to read; say so
            // rather than silently reporting a duplicate count of zero.
            eprintln!(
                "Warning: Could not determine history directory; \
                 duplicate detection skipped in dry-run."
            );
            (None, None)
        }
    } else {
        (None, None)
    };

    let result = match policy.hostname {
        Some(hostname) => {
            let overridden: Vec<ImportEntry> = entries
                .iter()
                .cloned()
                .map(|mut entry| {
                    entry.item.hostname = Some(hostname.to_owned());
                    entry
                })
                .collect();
            import_entries_dry_run(&overridden, r_dedup.as_ref(), shell_dedup.as_ref())
        }
        None => import_entries_dry_run(entries, r_dedup.as_ref(), shell_dedup.as_ref()),
    };
    Ok(result)
}

/// Create the destination directory if needed and open both databases.
pub(crate) fn open_targets(destination: &HistoryDbPaths) -> Result<ImportTargets> {
    // config::ensure_directories only creates XDG base dirs, not the history
    // subdirectory or custom --history-dir paths.
    fs::create_dir_all(&destination.directory).with_context(|| {
        format!(
            "Failed to create history directory: {}",
            destination.directory.display()
        )
    })?;

    Ok(ImportTargets {
        r_history: HistoryStore::open(destination.r.clone(), HistoryKind::R, None, None)
            .with_context(|| {
                format!(
                    "Failed to open R history database: {}",
                    destination.r.display()
                )
            })?,
        shell_history: HistoryStore::open(
            destination.shell.clone(),
            HistoryKind::Shell,
            None,
            None,
        )
        .with_context(|| {
            format!(
                "Failed to open shell history database: {}",
                destination.shell.display()
            )
        })?,
    })
}

/// Open the destination databases and import `entries` into them.
pub(crate) fn execute(
    entries: Vec<ImportEntry>,
    destination: &HistoryDbPaths,
    policy: ImportPolicy<'_>,
) -> Result<ImportResult> {
    let mut targets = open_targets(destination)?;
    import_entries(
        &mut targets,
        entries,
        policy.hostname,
        policy.skip_duplicates,
    )
}

/// Print an import summary under `heading`.
///
/// `duplicates_note` is appended to the duplicate count so each command can
/// point at the option that changes duplicate handling, if it has one.
pub(crate) fn print_summary(
    heading: &str,
    result: &ImportResult,
    hostname: Option<&str>,
    duplicates_note: &str,
) {
    println!("\n{heading}");
    if let Some(h) = hostname {
        println!("  Hostname:       {}", h);
    }
    println!("  R commands:     {}", result.r_imported);
    println!("  Shell commands: {}", result.shell_imported);
    println!("  Skipped:        {}", result.skipped);
    if result.duplicates_repaired > 0 {
        println!("  Duplicate repairs:  {}", result.duplicates_repaired);
    }
    if result.duplicates_skipped > 0 {
        println!(
            "  Duplicates:     {}{}",
            result.duplicates_skipped, duplicates_note
        );
    }

    if !result.warnings.is_empty() {
        println!("\nWarnings:");
        for warning in result.warnings.iter().take(10) {
            println!("  - {}", warning);
        }
        if result.warnings.len() > 10 {
            println!("  ... and {} more warnings", result.warnings.len() - 10);
        }
    }
}
