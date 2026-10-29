//! Handlers for `arf config`, `arf history`, and `arf ipc` subcommands.

use crate::app::config_load::load_config_or_warn;
use crate::cli::{ConfigAction, HistoryAction, ImportSource, IpcAction, RHelpAction};
use crate::config::{
    self, ConfigLoadError, config_file_path, init_config, load_config_from_path, mask_home_path,
};
use crate::history;
use crate::ipc;
use crate::pager;
use anyhow::{Context, Result};

/// Handle config subcommands.
pub(crate) fn handle_config_command(action: &ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Init { force } => {
            let path = init_config(*force)?;
            println!("Configuration file created at: {}", path.display());
            Ok(())
        }
        ConfigAction::Check { config: path } => handle_config_check(path.as_deref()),
    }
}

/// Handle `arf config check` — validate the config file and report errors.
fn handle_config_check(path: Option<&std::path::Path>) -> Result<()> {
    let config_path = if let Some(p) = path {
        p.to_path_buf()
    } else if let Some(p) = config_file_path() {
        p
    } else {
        anyhow::bail!("Could not determine config file path");
    };

    if !config_path.exists() {
        anyhow::bail!(
            "Config file not found: {}\nRun `arf config init` to create a default configuration file.",
            mask_home_path(&config_path)
        );
    }

    println!("Checking config file: {}", mask_home_path(&config_path));

    match load_config_from_path(&config_path) {
        Ok(mut config) => {
            println!("Config file is valid.");
            if let Some(warning) = config.history_migration_warning.take() {
                eprintln!("Warning: {warning}");
            }
            Ok(())
        }
        Err(ConfigLoadError::Parse { source, .. }) => {
            anyhow::bail!("Config file has errors:\n\n  {}", source);
        }
        Err(ConfigLoadError::Read { source, .. }) => {
            anyhow::bail!("Could not read config file: {}", source);
        }
        Err(ConfigLoadError::Validation { message, .. }) => {
            anyhow::bail!("Config file has errors:\n\n  {}", message);
        }
    }
}

pub(crate) fn handle_history_command(
    action: &HistoryAction,
    config_path: Option<&std::path::PathBuf>,
    cli_history_dir: Option<&std::path::PathBuf>,
) -> Result<()> {
    let config = load_config_or_warn(config_path);
    // `--no-history` configures the in-memory HistoryRuntime for interactive/headless sessions;
    // history subcommands create no runtime and reject that session-only option by scope.
    let mode = config::history_mode_with_overrides(
        &config.history.mode,
        cli_history_dir.map(|path| path.as_path()),
        false,
    );
    // `migrate` has fixed endpoints, so it never resolves or probes a location:
    // an unreadable default directory must not block the very command that
    // copies history out of it.
    if let HistoryAction::Migrate { dry_run } = action {
        let explicit_dir = match &mode {
            config::HistoryMode::Persistent { dir: Some(dir) } => Some(dir.as_path()),
            _ => None,
        };
        return handle_history_migrate(*dry_run, explicit_dir);
    }
    let history_location = config::resolved_history_location(&mode);
    if let Some(failure) = history_location.probe_failure() {
        anyhow::bail!("Cannot determine the history directory: {failure}");
    }

    match action {
        HistoryAction::Schema => pager::history_schema::print_schema(&history_location)
            .context("Failed to display history schema"),
        HistoryAction::Import {
            from,
            file,
            hostname,
            dry_run,
            import_duplicates,
            unified,
            r_table,
            shell_table,
        } => handle_history_import(
            *from,
            file.as_ref(),
            hostname.as_deref(),
            *dry_run,
            !import_duplicates,
            *unified,
            r_table,
            shell_table,
            history_location
                .directory()
                .map(std::path::Path::to_path_buf),
        ),
        HistoryAction::Migrate { .. } => unreachable!("migrate is handled before resolving"),
        HistoryAction::Export {
            file,
            r_table,
            shell_table,
        } => handle_history_export(file, r_table, shell_table, history_location.directory()),
    }
}

pub(crate) fn handle_ipc_command(action: &IpcAction) {
    match action {
        IpcAction::List => ipc::client::cmd_list(),
        IpcAction::Attach { pid, subscribe } => ipc::bridge::cmd_attach(*pid, subscribe),
        IpcAction::Eval {
            code,
            pid,
            visible,
            timeout,
        } => ipc::client::cmd_eval(code.as_deref(), *pid, *visible, *timeout),
        IpcAction::Send { code, pid } => ipc::client::cmd_send(code.as_deref(), *pid),
        IpcAction::Shutdown { pid } => ipc::client::cmd_shutdown(*pid),
        IpcAction::Interrupt { pid, execution_id } => {
            ipc::client::cmd_interrupt(*pid, *execution_id)
        }
        IpcAction::Session { pid } => ipc::client::cmd_session(*pid),
        IpcAction::History {
            limit,
            all_sessions,
            cwd,
            grep,
            since,
            pid,
        } => ipc::client::cmd_history(
            *pid,
            *limit,
            *all_sessions,
            cwd.as_deref(),
            grep.as_deref(),
            since.as_deref(),
        ),
        IpcAction::RHelp(action) => match action {
            RHelpAction::Search {
                query,
                package,
                limit,
                pid,
            } => ipc::client::cmd_r_help_search(*pid, query.as_deref(), package.as_deref(), *limit),
            RHelpAction::Show {
                topic,
                package,
                current_package,
                help_key,
                pid,
            } => ipc::client::cmd_r_help_show(
                *pid,
                topic,
                package.as_deref(),
                current_package.as_deref(),
                help_key.as_deref(),
            ),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_history_import(
    source: ImportSource,
    file: Option<&std::path::PathBuf>,
    hostname: Option<&str>,
    dry_run: bool,
    skip_duplicates: bool,
    unified: bool,
    r_table: &str,
    shell_table: &str,
    history_dir: Option<std::path::PathBuf>,
) -> Result<()> {
    use history::import::service::{self, HistoryDbPaths, ImportPolicy};
    use history::import::{
        default_r_history_path, default_radian_path, parse_arf_history, parse_r_history,
        parse_radian_history, parse_unified_arf_history,
    };

    // Determine source file path
    // Note: --from arf requires --file to avoid self-import (source = target)
    let source_path = match (source, file) {
        (_, Some(path)) => path.clone(),
        (ImportSource::Radian, None) => default_radian_path(),
        (ImportSource::R, None) => default_r_history_path(),
        (ImportSource::Arf, None) => {
            anyhow::bail!(
                "The --file option is required when importing from arf format.\n\
                 Example: arf history import --from arf --file /path/to/backup/r.db"
            );
        }
    };

    // Check if source file exists
    if !source_path.exists() {
        anyhow::bail!(
            "Source history file not found: {}\nSpecify the path with --file",
            source_path.display()
        );
    }

    println!("Importing from: {}", source_path.display());

    // Parse entries from source
    let parsed = match source {
        ImportSource::Radian => parse_radian_history(&source_path)?,
        ImportSource::R => parse_r_history(&source_path)?,
        ImportSource::Arf => {
            // Versioned artifact metadata takes precedence over the legacy filename/flag rules.
            let artifact = history::artifact::read_artifact_from_path(&source_path)?;
            let is_unified = should_parse_unified_import(artifact, unified, &source_path);

            if is_unified {
                // Unified export file - use table names to import both r and shell
                parse_unified_arf_history(&source_path, r_table, shell_table)?
            } else {
                // Traditional single-database import
                parse_arf_history(&source_path)?
            }
        }
    };
    let entries = parsed.entries;
    let parse_warnings = parsed.warnings;

    println!("Found {} entries to import", entries.len());

    let policy = ImportPolicy {
        hostname,
        skip_duplicates,
    };
    let duplicates_note = " (use --import-duplicates to import anyway)";

    // In dry-run mode, simulate the import
    if dry_run {
        let destination = history_dir.map(HistoryDbPaths::new);
        let mut result = service::preview(&entries, destination.as_ref(), policy)?;
        result.warnings.extend(parse_warnings);
        service::print_summary(
            "[Dry run] Would import:",
            &result,
            hostname,
            duplicates_note,
        );
        return Ok(());
    }

    // Determine target database paths (require history_dir for actual import)
    let destination = HistoryDbPaths::new(
        history_dir.ok_or_else(|| anyhow::anyhow!("Could not determine history directory"))?,
    );

    // Prevent self-import when using `--from arf` with `--file` pointing at the
    // same database as the target, which would duplicate history entries.
    if matches!(source, ImportSource::Arf)
        && let Some(kind) = service::shared_database(&source_path, &destination)
    {
        let label = match kind {
            history::artifact::HistoryKind::R => "R",
            history::artifact::HistoryKind::Shell => "shell",
        };
        anyhow::bail!(
            "Refusing to import from '{}' into itself ({} history database). \
             Please specify a different --file or history directory.",
            source_path.display(),
            label
        );
    }

    println!("Target databases:");
    println!("  R:     {}", destination.r.display());
    println!("  Shell: {}", destination.shell.display());

    let mut result = service::execute(entries, &destination, policy)?;
    result.warnings.extend(parse_warnings);
    service::print_summary("Import complete:", &result, hostname, duplicates_note);

    Ok(())
}

/// Handle `arf history migrate`.
///
/// `explicit_dir` is an explicitly configured history directory, if any. It
/// never changes the migration endpoints, which are always the platform
/// defaults; it is only reported so the user is not misled.
fn handle_history_migrate(dry_run: bool, explicit_dir: Option<&std::path::Path>) -> Result<()> {
    let dirs = config::default_history_dirs()
        .ok_or_else(|| anyhow::anyhow!("Could not determine the default history directories"))?;
    migrate_between_defaults(&dirs, dry_run, explicit_dir)
}

/// Migrate between the given default locations. A legacy location that cannot
/// be determined is an error, never "nothing to migrate": silence would hide
/// that history may exist somewhere arf cannot see.
fn migrate_between_defaults(
    dirs: &config::DefaultHistoryDirs,
    dry_run: bool,
    explicit_dir: Option<&std::path::Path>,
) -> Result<()> {
    use history::import::migrate::{migrate_history, report_outcome};

    let legacy = dirs.legacy.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "Could not determine the legacy default history directory, so there is nothing \
             to migrate from (the platform data directory is unavailable)"
        )
    })?;

    if let Some(dir) = explicit_dir {
        println!(
            "Note: an explicit history directory is configured ({}). It is ignored here: \
             `arf history migrate` only migrates between the default locations.",
            dir.display()
        );
    }

    if !crate::history::paths::same_directory(legacy, &dirs.current) {
        println!(
            "Close other running arf sessions before migrating, and keep them closed until it finishes.\n\
             The legacy location is kept; it is only read, never modified or deleted.\n"
        );
    }

    let outcome = migrate_history(legacy, &dirs.current, dry_run)?;
    report_outcome(&outcome)
}

fn handle_history_export(
    output_file: &std::path::Path,
    r_table: &str,
    shell_table: &str,
    history_dir: Option<&std::path::Path>,
) -> Result<()> {
    use history::export::export_history;

    let history_dir = history_dir
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("Could not determine history directory"))?;

    let r_path = history_dir.join("r.db");
    let shell_path = history_dir.join("shell.db");

    // Check if at least one database exists
    if !r_path.exists() && !shell_path.exists() {
        anyhow::bail!(
            "No history databases found in: {}\n\
             Expected r.db and/or shell.db",
            history_dir.display()
        );
    }

    println!("Exporting history to: {}", output_file.display());
    println!("Source databases:");
    if r_path.exists() {
        println!("  R:     {} (table: {})", r_path.display(), r_table);
    }
    if shell_path.exists() {
        println!("  Shell: {} (table: {})", shell_path.display(), shell_table);
    }

    let result = export_history(&r_path, &shell_path, output_file, r_table, shell_table)?;

    println!("\nExport complete:");
    println!("  R commands:     {}", result.r_exported);
    println!("  Shell commands: {}", result.shell_exported);

    Ok(())
}

fn should_parse_unified_import(
    artifact: history::artifact::HistoryArtifact,
    unified_flag: bool,
    source_path: &std::path::Path,
) -> bool {
    match artifact {
        history::artifact::HistoryArtifact::History(_) => false,
        history::artifact::HistoryArtifact::Export => true,
        history::artifact::HistoryArtifact::Legacy => {
            unified_flag
                || !matches!(
                    source_path.file_name().and_then(|name| name.to_str()),
                    Some("r.db" | "shell.db")
                )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::artifact::{HistoryArtifact, HistoryKind};

    #[test]
    fn migrate_with_unknown_legacy_directory_is_an_error_and_touches_nothing() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("state").join("arf").join("history");
        let dirs = config::DefaultHistoryDirs {
            current: current.clone(),
            legacy: None,
        };
        let error = migrate_between_defaults(&dirs, false, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("legacy default history directory"),
            "unexpected error: {error}"
        );
        assert!(!current.exists());
    }

    #[test]
    fn versioned_artifact_metadata_precedes_filename_and_unified_flag() {
        assert!(!should_parse_unified_import(
            HistoryArtifact::History(HistoryKind::Shell),
            true,
            std::path::Path::new("renamed.db"),
        ));
        assert!(should_parse_unified_import(
            HistoryArtifact::Export,
            false,
            std::path::Path::new("shell.db"),
        ));
    }

    #[test]
    fn legacy_detection_retains_filename_and_unified_flag_behavior() {
        assert!(!should_parse_unified_import(
            HistoryArtifact::Legacy,
            false,
            std::path::Path::new("r.db"),
        ));
        assert!(should_parse_unified_import(
            HistoryArtifact::Legacy,
            false,
            std::path::Path::new("backup.db"),
        ));
        assert!(should_parse_unified_import(
            HistoryArtifact::Legacy,
            true,
            std::path::Path::new("r.db"),
        ));
    }
}
