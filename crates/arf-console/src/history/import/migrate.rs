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
use super::{ImportResult, ParsedImport, parse_arf_history};

/// What a migration run found and did.
#[derive(Debug)]
pub(crate) enum MigrationOutcome {
    /// Legacy and current are the same directory; nothing was touched.
    SameLocation(PathBuf),
    /// The legacy directory holds neither `r.db` nor `shell.db`.
    NoLegacyHistory(PathBuf),
    Migrated(Box<MigrationReport>),
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
    /// Current databases in which the completed migration could not be
    /// recorded. Without the record, startup keeps reporting the legacy
    /// history as not yet copied, so this makes the migration incomplete.
    pub(crate) record_failures: usize,
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
fn read_legacy_database(path: &Path, kind: HistoryKind) -> Result<ParsedImport> {
    if let HistoryArtifact::History(actual) = artifact::read_artifact_from_path(path)?
        && actual != kind
    {
        bail!(
            "expected a '{}' history database but its metadata says '{}'",
            kind.as_str(),
            actual.as_str()
        );
    }
    parse_arf_history(path)
}

/// Mark the current databases of the kinds the legacy directory had as
/// migrated so startup stops reporting that legacy history as not yet copied.
/// Kinds the legacy directory lacked are left unmarked, and a record an earlier
/// migration left for them is cleared, so legacy history of that kind created
/// later is still reported. Failures are returned as a count (with a warning
/// each) rather than hidden: the migration is incomplete until the record
/// exists, and a rerun retries it.
fn record_completed_migration(
    current: &HistoryDbPaths,
    legacy_dir: &Path,
    legacy_has_r: bool,
    legacy_has_shell: bool,
    warnings: &mut Vec<String>,
) -> usize {
    let mut failures = 0;
    for (kind, present) in [
        (HistoryKind::R, legacy_has_r),
        (HistoryKind::Shell, legacy_has_shell),
    ] {
        let recorded = if present {
            artifact::record_legacy_migration(current.path(kind), kind, legacy_dir)
        } else {
            artifact::clear_legacy_migration(current.path(kind), legacy_dir)
        };
        if let Err(error) = recorded {
            failures += 1;
            warnings.push(format!(
                "Could not record the completed migration: {error:#}"
            ));
        }
    }
    failures
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
        if !dry_run {
            // Records an earlier migration left would otherwise hide legacy
            // databases created here later.
            for kind in [HistoryKind::R, HistoryKind::Shell] {
                artifact::clear_legacy_migration(current.path(kind), &legacy.directory)?;
            }
        }
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
    let mut parse_warnings = Vec::new();
    for (kind, present) in [
        (HistoryKind::R, legacy_has_r),
        (HistoryKind::Shell, legacy_has_shell),
    ] {
        if present {
            let path = legacy.path(kind);
            let parsed = read_legacy_database(path, kind).with_context(|| {
                format!(
                    "Failed to read legacy {} history database: {}\n\
                     Nothing was written to the current location. Fix or move the file and rerun.",
                    kind.as_str(),
                    path.display()
                )
            })?;
            entries.extend(parsed.entries);
            parse_warnings.extend(parsed.warnings);
        }
    }

    let current_had_r = database_exists(&current.r)?;
    let current_had_shell = database_exists(&current.shell)?;
    let policy = ImportPolicy {
        hostname: None,
        skip_duplicates: true,
    };
    let mut result = if dry_run {
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

    result.warnings.extend(parse_warnings);

    let record_failures = if !dry_run && result.write_failures == 0 {
        record_completed_migration(
            &current,
            &legacy.directory,
            legacy_has_r,
            legacy_has_shell,
            &mut result.warnings,
        )
    } else {
        0
    };

    Ok(MigrationOutcome::Migrated(Box::new(MigrationReport {
        legacy,
        current,
        legacy_has_r,
        legacy_has_shell,
        current_had_r,
        current_had_shell,
        dry_run,
        result,
        record_failures,
    })))
}

/// Print what [`migrate_history`] found and did.
///
/// Returns an error when any row could not be written to the current
/// location, after the summary and warnings are shown. The import engine
/// reports row-level write errors as warnings so one bad row does not stop the
/// rest; a migration must not call that success.
pub(crate) fn report_outcome(outcome: &MigrationOutcome) -> Result<()> {
    let report = match outcome {
        MigrationOutcome::SameLocation(dir) => {
            println!(
                "Nothing to migrate: legacy and current default locations are the same ({}).",
                dir.display()
            );
            return Ok(());
        }
        MigrationOutcome::NoLegacyHistory(dir) => {
            println!(
                "Nothing to migrate: no history databases (r.db, shell.db) found in the legacy location ({}).",
                dir.display()
            );
            return Ok(());
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

    let incomplete = report.result.write_failures > 0 || report.record_failures > 0;
    let heading = if report.dry_run {
        "[Dry run] Would migrate:"
    } else if incomplete {
        "Migration incomplete:"
    } else {
        "Migration complete:"
    };
    service::print_summary(
        heading,
        &report.result,
        None,
        " (already in the current location)",
    );

    if incomplete {
        let mut problems = Vec::new();
        if report.result.write_failures > 0 {
            problems.push(format!(
                "{} row(s) could not be written",
                report.result.write_failures
            ));
        }
        if report.record_failures > 0 {
            problems.push(format!(
                "the completed migration could not be recorded in {} database(s)",
                report.record_failures
            ));
        }
        bail!(
            "Migration into {} was incomplete: {} (see the warnings above).\n\
             The legacy location {} was not modified. Rerunning `arf history migrate` is safe: \
             entries already copied are skipped.",
            report.current.directory.display(),
            problems.join("; "),
            report.legacy.directory.display()
        );
    }
    if report.dry_run {
        println!("\nDry run only: no history was written.");
    } else {
        println!(
            "\nThe legacy location was kept unchanged. Arf sessions that were open during the \
             migration may still have written to it; close them and rerun this command to copy \
             any remaining entries (rerunning is safe). Once you have confirmed your history in \
             the new location, you may delete the legacy directory {} manually, but arf \
             versions from before the default location changed still use it.",
            report.legacy.directory.display()
        );
    }
    Ok(())
}
