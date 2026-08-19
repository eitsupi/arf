//! Handlers for `arf config`, `arf history`, and `arf ipc` subcommands.

use crate::app::config_load::load_config_or_warn;
use crate::cli::{ConfigAction, HistoryAction, ImportSource, IpcAction};
use crate::config::{
    self, ConfigLoadError, config_file_path, init_config, load_config_from_path, mask_home_path,
};
use crate::history;
use crate::ipc;
use crate::pager;
use anyhow::{Context, Result};
use std::fs;

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
        Ok(_) => {
            println!("Config file is valid.");
            Ok(())
        }
        Err(ConfigLoadError::Parse { source, .. }) => {
            anyhow::bail!("Config file has errors:\n\n  {}", source);
        }
        Err(ConfigLoadError::Read { source, .. }) => {
            anyhow::bail!("Could not read config file: {}", source);
        }
    }
}

pub(crate) fn handle_history_command(
    action: &HistoryAction,
    config_path: Option<&std::path::PathBuf>,
    cli_history_dir: Option<&std::path::PathBuf>,
) -> Result<()> {
    match action {
        HistoryAction::Schema => {
            pager::history_schema::print_schema().context("Failed to display history schema")
        }
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
            config_path,
            cli_history_dir,
        ),
        HistoryAction::Export {
            file,
            r_table,
            shell_table,
        } => handle_history_export(file, r_table, shell_table, config_path, cli_history_dir),
    }
}

pub(crate) fn handle_ipc_command(action: &IpcAction) {
    match action {
        IpcAction::List => ipc::client::cmd_list(),
        IpcAction::Eval {
            code,
            pid,
            visible,
            timeout,
        } => ipc::client::cmd_eval(code.as_deref(), *pid, *visible, *timeout),
        IpcAction::Send { code, pid } => ipc::client::cmd_send(code.as_deref(), *pid),
        IpcAction::Shutdown { pid } => ipc::client::cmd_shutdown(*pid),
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
    config_path: Option<&std::path::PathBuf>,
    cli_history_dir: Option<&std::path::PathBuf>,
) -> Result<()> {
    use history::import::{
        DedupSet, default_r_history_path, default_radian_path, import_entries,
        import_entries_dry_run, parse_arf_history, parse_r_history, parse_radian_history,
        parse_unified_arf_history,
    };

    // Load config (respecting --config flag if provided)
    let config = load_config_or_warn(config_path);

    // Resolve effective history directory (CLI --history-dir takes precedence)
    // Required for actual imports and for dry-run with dedup (needs DB access)
    let history_dir = cli_history_dir
        .cloned()
        .or_else(|| config::history_dir_for_mode(&config.history.mode));

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
            // Determine if this is a unified export file or a single-database file.
            // --unified flag forces unified mode; otherwise infer from filename.
            let is_unified = unified || {
                let filename = source_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                filename != "r.db" && filename != "shell.db"
            };

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

    // In dry-run mode, simulate the import
    if dry_run {
        // Build dedup sets if duplicate skipping is enabled (requires DB access).
        // Each database is checked independently so dedup works even if only
        // one of the two target databases exists.
        let (r_dedup, shell_dedup) = if skip_duplicates {
            if let Some(ref history_dir) = history_dir {
                let r_path = history_dir.join("r.db");
                let shell_path = history_dir.join("shell.db");
                let r_dedup = if r_path.exists() {
                    Some(DedupSet::from_db(&r_path)?)
                } else {
                    None
                };
                let shell_dedup = if shell_path.exists() {
                    Some(DedupSet::from_db(&shell_path)?)
                } else {
                    None
                };
                (r_dedup, shell_dedup)
            } else {
                // history_dir could not be resolved (no config, no XDG default).
                // Dedup is silently skipped; warn the user so they know the
                // duplicate count is not available.
                eprintln!(
                    "Warning: Could not determine history directory; \
                     duplicate detection skipped in dry-run."
                );
                (None, None)
            }
        } else {
            (None, None)
        };

        let dry_entries = if let Some(hostname) = hostname {
            entries
                .iter()
                .cloned()
                .map(|mut entry| {
                    entry.item.hostname = Some(hostname.to_owned());
                    entry
                })
                .collect()
        } else {
            entries.clone()
        };
        let mut result =
            import_entries_dry_run(&dry_entries, r_dedup.as_ref(), shell_dedup.as_ref());
        result.warnings.extend(parse_warnings);

        println!("\n[Dry run] Would import:");
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
                "  Duplicates:     {} (use --import-duplicates to import anyway)",
                result.duplicates_skipped
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

        return Ok(());
    }

    // Determine target database paths (require history_dir for actual import)
    let history_dir =
        history_dir.ok_or_else(|| anyhow::anyhow!("Could not determine history directory"))?;
    let r_path = history_dir.join("r.db");
    let shell_path = history_dir.join("shell.db");

    // Prevent self-import when using `--from arf` with `--file` pointing at the
    // same database as the target, which would duplicate history entries.
    if matches!(source, ImportSource::Arf)
        && let Ok(source_canon) = fs::canonicalize(&source_path)
    {
        if fs::canonicalize(&r_path).is_ok_and(|r_canon| source_canon == r_canon) {
            anyhow::bail!(
                "Refusing to import from '{}' into itself (R history database). \
                 Please specify a different --file or history directory.",
                source_path.display()
            );
        }
        if fs::canonicalize(&shell_path).is_ok_and(|shell_canon| source_canon == shell_canon) {
            anyhow::bail!(
                "Refusing to import from '{}' into itself (shell history database). \
                 Please specify a different --file or history directory.",
                source_path.display()
            );
        }
    }

    // Ensure the history directory exists (config::ensure_directories only creates XDG base dirs,
    // not the history subdirectory or custom --history-dir paths)
    fs::create_dir_all(&history_dir).with_context(|| {
        format!(
            "Failed to create history directory: {}",
            history_dir.display()
        )
    })?;

    println!("Target databases:");
    println!("  R:     {}", r_path.display());
    println!("  Shell: {}", shell_path.display());

    let mut targets = history::import::ImportTargets {
        r_history: history::HistoryStore::open(r_path, None, None)
            .context("Failed to open R history database")?,
        shell_history: history::HistoryStore::open(shell_path, None, None)
            .context("Failed to open shell history database")?,
    };

    // Import entries
    let mut result = import_entries(&mut targets, entries, hostname, skip_duplicates)?;
    result.warnings.extend(parse_warnings);

    println!("\nImport complete:");
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
            "  Duplicates:     {} (use --import-duplicates to import anyway)",
            result.duplicates_skipped
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

    Ok(())
}

fn handle_history_export(
    output_file: &std::path::Path,
    r_table: &str,
    shell_table: &str,
    config_path: Option<&std::path::PathBuf>,
    cli_history_dir: Option<&std::path::PathBuf>,
) -> Result<()> {
    use history::export::export_history;

    // Load config (respecting --config flag if provided)
    let config = load_config_or_warn(config_path);

    // Resolve effective history directory
    let history_dir = cli_history_dir
        .cloned()
        .or_else(|| config::history_dir_for_mode(&config.history.mode))
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
