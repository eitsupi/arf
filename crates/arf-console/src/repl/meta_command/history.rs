//! `:history` meta command.

use super::{MetaCommandContext, MetaCommandResult};
use crate::history::{HistoryRuntime, HistoryStore};
use crate::meta_command_catalog::HISTORY_SUBCOMMANDS;
use crate::pager::HistoryDbMode;
use crate::repl::shell::confirm_action;
use crate::repl::{ARF_PREFIX, arf_println};

/// Handle `:history [browse|clear|schema] [target]`.
pub(super) fn handle_history(ctx: &MetaCommandContext<'_>, parts: &[&str]) -> MetaCommandResult {
    let subcmd = parts.get(1).copied().unwrap_or("");
    match subcmd {
        "browse" => {
            let target = parts.get(2).copied().unwrap_or("");
            process_history_browse(
                ctx.r_history,
                ctx.shell_history,
                target,
                ctx.prompt_config.is_shell_enabled(),
            )
        }
        "clear" => {
            let target = parts.get(2).copied().unwrap_or("");
            process_history_clear(
                ctx.r_history,
                ctx.shell_history,
                target,
                ctx.prompt_config.is_shell_enabled(),
            )
        }
        "schema" => MetaCommandResult::ShowHistorySchema,
        "" => {
            arf_println!("Usage: :history <subcommand>");
            let width = HISTORY_SUBCOMMANDS
                .iter()
                .map(|subcommand| subcommand.name.len())
                .max()
                .unwrap_or(0);
            for subcommand in HISTORY_SUBCOMMANDS {
                println!(
                    "#   {:<width$} - {}",
                    subcommand.name, subcommand.description
                );
            }
            MetaCommandResult::Handled
        }
        _ => {
            arf_println!(
                "Unknown history subcommand: {}. Use :history for help",
                subcmd
            );
            MetaCommandResult::Handled
        }
    }
}

/// Process :history browse command.
fn process_history_browse(
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    target: &str,
    is_shell_mode: bool,
) -> MetaCommandResult {
    // Determine which database to browse
    let (mode, runtime) = match target {
        "" => {
            // Default: browse based on current mode
            if is_shell_mode {
                (HistoryDbMode::Shell, shell_history)
            } else {
                (HistoryDbMode::R, r_history)
            }
        }
        "r" | "R" => (HistoryDbMode::R, r_history),
        "shell" => (HistoryDbMode::Shell, shell_history),
        _ => {
            arf_println!("Unknown target: {}. Use r or shell.", target);
            return MetaCommandResult::Handled;
        }
    };

    let Some(store) = runtime.store() else {
        arf_println!("History is unavailable for {} mode.", mode.display_name());
        return MetaCommandResult::Handled;
    };

    MetaCommandResult::ShowHistoryBrowser { store, mode }
}

/// Process :history clear command.
fn process_history_clear(
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    target: &str,
    is_shell_mode: bool,
) -> MetaCommandResult {
    // Determine what to clear based on target
    let clear_target = match target {
        "" => {
            // Default: clear based on current mode
            if is_shell_mode { "shell" } else { "r" }
        }
        "r" | "R" => "r",
        "shell" => "shell",
        "all" => "all",
        _ => {
            arf_println!("Unknown target: {}. Use r, shell, or all.", target);
            return MetaCommandResult::Handled;
        }
    };

    // Collect paths to clear based on target
    let runtimes: Vec<(&str, &HistoryRuntime)> = match clear_target {
        "r" => vec![("R", r_history)],
        "shell" => vec![("Shell", shell_history)],
        "all" => {
            vec![("R", r_history), ("Shell", shell_history)]
        }
        _ => unreachable!(),
    };

    let stores = dedup_history_stores(
        runtimes
            .into_iter()
            .filter_map(|(name, runtime)| runtime.store().map(|store| (name, store)))
            .collect(),
    );

    if stores.is_empty() {
        arf_println!("History is unavailable.");
        return MetaCommandResult::Handled;
    }

    // Count total entries across all targeted databases
    let mut total_count = 0i64;
    let mut counts: Vec<(&str, i64)> = Vec::new();

    for (name, store) in &stores {
        match store.count_all() {
            Ok(count) => {
                counts.push((name, count));
                total_count += count;
            }
            Err(error) => arf_println!("Failed to read {} history: {}", name, error),
        }
    }

    if total_count == 0 {
        arf_println!("History is already empty.");
        return MetaCommandResult::Handled;
    }

    // Show what will be cleared
    if counts.len() == 1 {
        arf_println!("{} history: {} entries", counts[0].0, counts[0].1);
    } else {
        for (name, count) in &counts {
            arf_println!("{} history: {} entries", name, count);
        }
        arf_println!("Total: {} entries", total_count);
    }

    // Confirm before clearing
    let prompt = format!("{} Clear {} history entries?", ARF_PREFIX, total_count);
    if !confirm_action(&prompt) {
        arf_println!("Cancelled.");
        return MetaCommandResult::Handled;
    }

    MetaCommandResult::ClearHistory { stores }
}

pub(super) fn dedup_history_stores(stores: Vec<(&str, HistoryStore)>) -> Vec<(&str, HistoryStore)> {
    let mut unique: Vec<(&str, HistoryStore)> = Vec::new();
    for (name, store) in stores {
        if unique
            .iter()
            .any(|(_, existing)| existing.same_owner(&store))
        {
            continue;
        }
        unique.push((name, store));
    }
    unique
}
