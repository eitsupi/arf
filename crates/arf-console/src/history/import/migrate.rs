//! Copying history from the legacy default location to the current one.
//!
//! Both directories are parameters so the logic can be exercised against
//! arbitrary directories; the command layer supplies the platform defaults.
//! The legacy databases are only ever opened read-only.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

use super::super::artifact::{self, HistoryArtifact, HistoryKind};
use super::service::{self, HistoryDbPaths, ImportPolicy};
use super::{ImportEntry, ImportResult, parse_arf_history};

/// What a migration run found and did.
#[derive(Debug)]
pub(crate) enum MigrationOutcome {
    /// Legacy and current are the same directory; nothing was touched.
    SameLocation(PathBuf),
    /// The legacy directory holds neither `r.db` nor `shell.db`.
    NoLegacyHistory(PathBuf),
    Migrated(MigrationReport),
}

#[derive(Debug)]
pub(crate) struct MigrationReport {
    pub(crate) legacy: HistoryDbPaths,
    pub(crate) current: HistoryDbPaths,
    pub(crate) legacy_has_r: bool,
    pub(crate) legacy_has_shell: bool,
    pub(crate) current_had_r: bool,
    pub(crate) current_had_shell: bool,
    pub(crate) dry_run: bool,
    pub(crate) result: ImportResult,
}

fn same_directory(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (fs::canonicalize(a), fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Check for a database file, reporting I/O and permission errors instead of
/// treating them as absence.
fn database_exists(path: &Path) -> Result<bool> {
    path.try_exists().with_context(|| {
        format!(
            "Failed to inspect history database path: {}",
            path.display()
        )
    })
}

/// Read one legacy database, insisting it is the kind its filename promises.
fn read_legacy_database(path: &Path, kind: HistoryKind) -> Result<Vec<ImportEntry>> {
    if let HistoryArtifact::History(actual) = artifact::read_artifact_from_path(path)?
        && actual != kind
    {
        bail!(
            "expected a '{}' history database but its metadata says '{}'",
            kind.as_str(),
            actual.as_str()
        );
    }
    let parsed = parse_arf_history(path)?;
    Ok(parsed.entries)
}

/// Copy history from `legacy_dir` into `current_dir` with duplicate detection.
///
/// With `dry_run`, nothing is created or modified. Source databases are read
/// before any destination file is touched, so an unreadable source leaves the
/// destination exactly as it was.
pub(crate) fn migrate_history(
    legacy_dir: &Path,
    current_dir: &Path,
    dry_run: bool,
) -> Result<MigrationOutcome> {
    if same_directory(legacy_dir, current_dir) {
        return Ok(MigrationOutcome::SameLocation(current_dir.to_path_buf()));
    }

    let legacy = HistoryDbPaths::new(legacy_dir.to_path_buf());
    let current = HistoryDbPaths::new(current_dir.to_path_buf());
    let legacy_has_r = database_exists(&legacy.r)?;
    let legacy_has_shell = database_exists(&legacy.shell)?;
    if !legacy_has_r && !legacy_has_shell {
        return Ok(MigrationOutcome::NoLegacyHistory(legacy.directory));
    }

    for kind in [HistoryKind::R, HistoryKind::Shell] {
        if service::shared_database(legacy.path(kind), &current).is_some() {
            bail!(
                "Legacy database '{}' is the same file as the current one '{}'; \
                 nothing to migrate",
                legacy.path(kind).display(),
                current.path(kind).display()
            );
        }
    }

    let mut entries = Vec::new();
    for (kind, present) in [
        (HistoryKind::R, legacy_has_r),
        (HistoryKind::Shell, legacy_has_shell),
    ] {
        if present {
            let path = legacy.path(kind);
            entries.extend(read_legacy_database(path, kind).with_context(|| {
                format!(
                    "Failed to read legacy {} history database: {}\n\
                     Nothing was written to the current location. Fix or move the file and rerun.",
                    kind.as_str(),
                    path.display()
                )
            })?);
        }
    }

    let current_had_r = database_exists(&current.r)?;
    let current_had_shell = database_exists(&current.shell)?;
    let policy = ImportPolicy {
        hostname: None,
        skip_duplicates: true,
    };
    let result = if dry_run {
        service::preview(&entries, Some(&current), policy).with_context(|| {
            format!(
                "Failed to read current history in: {}",
                current.directory.display()
            )
        })?
    } else {
        service::execute(entries, &current, policy).with_context(|| {
            format!(
                "Migration into {} failed; the legacy location {} was not modified.\n\
                 Rerunning `arf history migrate` is safe: entries already copied are skipped.",
                current.directory.display(),
                legacy.directory.display()
            )
        })?
    };

    Ok(MigrationOutcome::Migrated(MigrationReport {
        legacy,
        current,
        legacy_has_r,
        legacy_has_shell,
        current_had_r,
        current_had_shell,
        dry_run,
        result,
    }))
}

/// Print what [`migrate_history`] found and did.
pub(crate) fn print_outcome(outcome: &MigrationOutcome) {
    let report = match outcome {
        MigrationOutcome::SameLocation(dir) => {
            println!(
                "Nothing to migrate: legacy and current default locations are the same ({}).",
                dir.display()
            );
            return;
        }
        MigrationOutcome::NoLegacyHistory(dir) => {
            println!(
                "Nothing to migrate: no history databases (r.db, shell.db) found in the legacy location ({}).",
                dir.display()
            );
            return;
        }
        MigrationOutcome::Migrated(report) => report,
    };

    let found = match (report.legacy_has_r, report.legacy_has_shell) {
        (true, true) => "R and shell history",
        (true, false) => "R history only",
        (false, true) => "shell history only",
        (false, false) => unreachable!("migration requires at least one legacy database"),
    };
    println!("Legacy location:  {}", report.legacy.directory.display());
    println!("  Found:          {found}");
    println!("Current location: {}", report.current.directory.display());
    let existing = match (report.current_had_r, report.current_had_shell) {
        (true, true) => "R and shell history (duplicates are skipped)",
        (true, false) => "R history (duplicates are skipped)",
        (false, true) => "shell history (duplicates are skipped)",
        (false, false) if report.dry_run => "none (would be created)",
        (false, false) => "none (created)",
    };
    println!("  Existing:       {existing}");

    let heading = if report.dry_run {
        "[Dry run] Would migrate:"
    } else {
        "Migration complete:"
    };
    service::print_summary(
        heading,
        &report.result,
        None,
        " (already in the current location)",
    );
}
