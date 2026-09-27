//! Meta command processing.

use crate::completion::path::expand_tilde;
use crate::config::{RSourceStatus, ReprexMode};
use crate::external::formatter;
use crate::history::{HistoryRuntime, HistoryStore};
use crate::pager::HistoryDbMode;
use std::path::PathBuf;

use super::reprex::ReprexRuntime;
use super::shell::confirm_action;
use super::state::PromptRuntimeConfig;
use super::{ARF_PREFIX, arf_println};

/// Result of processing a meta command.
pub enum MetaCommandResult {
    /// Command was handled, continue with new prompt
    Handled,
    /// User wants to exit
    Exit,
    /// Unknown command
    Unknown(String),
    /// Shell command was executed inline (for :system)
    ShellExecuted,
    /// Restart the process with optional R version
    Restart(Option<String>),
    /// Open the help browser with the given query (caller runs pager)
    ShowHelpBrowser(String),
    /// Display session info (caller runs pager)
    ShowSessionInfo,
    /// Display changelog (caller runs pager)
    ShowChangelog,
    /// Open the history browser (caller runs pager)
    ShowHistoryBrowser {
        store: HistoryStore,
        mode: HistoryDbMode,
    },
    /// Clear stores after the caller has finalized the command provenance.
    ClearHistory {
        stores: Vec<(&'static str, HistoryStore)>,
    },
    /// Display history schema (caller runs pager)
    ShowHistorySchema,
}

/// Process a meta command (starting with `:`) and return the result.
#[allow(clippy::too_many_arguments)]
pub fn process_meta_command(
    input: &str,
    prompt_config: &mut PromptRuntimeConfig,
    reprex: &mut ReprexRuntime,
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    r_source_status: &RSourceStatus,
    dir_stack: &mut Vec<PathBuf>,
    history_session_id: Option<i64>,
    r_home: Option<&std::path::Path>,
) -> Option<MetaCommandResult> {
    let trimmed = input.trim();
    if !trimmed.starts_with(':') {
        return None;
    }

    let parts: Vec<&str> = trimmed[1..].split_whitespace().collect();
    let cmd = parts.first().copied().unwrap_or("");

    match cmd {
        "reprex" if parts.len() == 2 => match parts[1] {
            "on" => {
                reprex.set_mode(ReprexMode::On);
                arf_println!("Reprex: on");
                Some(MetaCommandResult::Handled)
            }
            "off" => {
                reprex.set_mode(ReprexMode::Off);
                arf_println!("Reprex: off");
                Some(MetaCommandResult::Handled)
            }
            "format" => {
                if reprex.mode != ReprexMode::Format {
                    let resolved = formatter::resolve_formatter(reprex.formatter_selector);
                    if let Some(backend) = resolved {
                        reprex.formatter = Some(backend);
                        reprex.set_mode(ReprexMode::Format);
                        arf_println!("Reprex: format");
                    } else {
                        arf_println!(
                            "{}",
                            formatter::unavailable_message(
                                reprex.formatter_selector,
                                formatter::FormatterUnavailableContext::MetaCommand
                            )
                        );
                    }
                } else {
                    // Repeating the command is idempotent and does not probe
                    // the formatter again.
                    reprex.set_mode(ReprexMode::Format);
                    arf_println!("Reprex: format");
                }
                Some(MetaCommandResult::Handled)
            }
            _ => {
                arf_println!("Usage: :reprex on|off|format");
                Some(MetaCommandResult::Handled)
            }
        },
        "reprex" => {
            arf_println!("Usage: :reprex on|off|format");
            Some(MetaCommandResult::Handled)
        }
        "shell" => {
            prompt_config.set_shell(true);
            arf_println!("Shell mode enabled. Type :r to return to R.");
            Some(MetaCommandResult::Handled)
        }
        "r" | "R" => {
            if prompt_config.is_shell_enabled() {
                prompt_config.set_shell(false);
                arf_println!("Returned to R mode.");
            } else {
                arf_println!("Already in R mode.");
            }
            Some(MetaCommandResult::Handled)
        }
        "system" => {
            // Execute the rest of the input as a shell command
            let shell_cmd = trimmed[1..].strip_prefix("system").unwrap_or("").trim();
            if shell_cmd.is_empty() {
                arf_println!("Usage: :system <command>");
            } else {
                if let Some(hint) = dir_command_hint(shell_cmd) {
                    arf_println!("{}", hint);
                }
                super::shell::execute_shell_command(shell_cmd);
            }
            Some(MetaCommandResult::ShellExecuted)
        }
        "restart" | "restart!" => {
            let force = cmd == "restart!";
            if force
                || confirm_action(&format!(
                    "{} Restart R session? Current session will be lost.",
                    ARF_PREFIX
                ))
            {
                arf_println!("Restarting R session...");
                Some(MetaCommandResult::Restart(None))
            } else {
                arf_println!("Restart cancelled.");
                Some(MetaCommandResult::Handled)
            }
        }
        "switch" | "switch!" => {
            // :switch requires rig to be enabled at startup
            if !r_source_status.rig_enabled() {
                arf_println!("Error: :switch requires rig to be available at startup.");
                arf_println!(
                    r#"Start arf with r_source = "auto" (with rig installed) or r_source = "rig"."#
                );
                return Some(MetaCommandResult::Handled);
            }

            // Extract the complete version argument, since version ranges may
            // contain spaces (for example, ">=4.3, <5.0").
            let version = trimmed[1..]
                .trim_start()
                .strip_prefix(cmd)
                .map(str::trim)
                .filter(|version| !version.is_empty())
                .map(str::to_owned);
            if version.is_none() {
                arf_println!("Usage: :{cmd} <version>");
                arf_println!("Example: :{cmd} 4.4 or :{cmd} release");
                return Some(MetaCommandResult::Handled);
            }
            let force = cmd == "switch!";
            let ver = version.as_ref().unwrap();
            if force || confirm_action(&format!("Restart with R {}?", ver)) {
                arf_println!("Restarting with R {}...", ver);
                Some(MetaCommandResult::Restart(version))
            } else {
                arf_println!("Switch cancelled.");
                Some(MetaCommandResult::Handled)
            }
        }
        "history" => {
            let subcmd = parts.get(1).copied().unwrap_or("");
            match subcmd {
                "browse" => {
                    let target = parts.get(2).copied().unwrap_or("");
                    process_history_browse(
                        r_history,
                        shell_history,
                        target,
                        prompt_config.is_shell_enabled(),
                    )
                }
                "clear" => {
                    let target = parts.get(2).copied().unwrap_or("");
                    process_history_clear(
                        r_history,
                        shell_history,
                        target,
                        prompt_config.is_shell_enabled(),
                    )
                }
                "schema" => Some(MetaCommandResult::ShowHistorySchema),
                "" => {
                    arf_println!("Usage: :history <subcommand>");
                    println!("#   browse - Browse and manage command history");
                    println!("#   clear  - Clear command history");
                    println!("#   schema - Display database schema and R examples");
                    Some(MetaCommandResult::Handled)
                }
                _ => {
                    arf_println!(
                        "Unknown history subcommand: {}. Use :history for help",
                        subcmd
                    );
                    Some(MetaCommandResult::Handled)
                }
            }
        }
        "help" | "h" => {
            // Fuzzy help search for R documentation
            // Inspired by the felp package: https://github.com/atusy/felp
            let query = parts.get(1..).map(|p| p.join(" ")).unwrap_or_default();
            Some(MetaCommandResult::ShowHelpBrowser(query))
        }
        "info" | "session" => Some(MetaCommandResult::ShowSessionInfo),
        "changelog" => Some(MetaCommandResult::ShowChangelog),
        "cd" => {
            let path_arg = trimmed[1..].strip_prefix("cd").unwrap_or("").trim();
            match meta_cd(path_arg) {
                Ok(cwd) => arf_println!("{}", cwd.display()),
                Err(e) => arf_println!("cd: {}", e),
            }
            Some(MetaCommandResult::Handled)
        }
        "pushd" => {
            let path_arg = trimmed[1..].strip_prefix("pushd").unwrap_or("").trim();
            match meta_pushd(dir_stack, path_arg) {
                Ok(cwd) => arf_println!("{}", cwd.display()),
                Err(e) => arf_println!("pushd: {}", e),
            }
            Some(MetaCommandResult::Handled)
        }
        "popd" => {
            match meta_popd(dir_stack) {
                Ok(cwd) => arf_println!("{}", cwd.display()),
                Err(e) => arf_println!("popd: {}", e),
            }
            Some(MetaCommandResult::Handled)
        }
        "ipc" => {
            let subcmd = parts.get(1).copied().unwrap_or("status");
            match subcmd {
                "start" => match crate::ipc::start_server(
                    None,
                    r_home.map(|path| path.display().to_string()),
                    None,
                    history_session_id,
                    crate::ipc::session::SessionType::Interactive,
                ) {
                    Ok(session) => {
                        arf_println!("IPC server started: {}", session.socket_path)
                    }
                    Err(e) => arf_println!("Failed to start IPC server: {}", e),
                },
                "stop" => {
                    crate::ipc::stop_server();
                    arf_println!("IPC server stopped.");
                }
                "status" => {
                    let sessions = crate::ipc::session::list_sessions();
                    let my_pid = std::process::id();
                    let my_session = sessions.iter().find(|s| s.pid == my_pid);
                    if let Some(session) = my_session {
                        arf_println!("IPC server is running.");
                        println!("#   Socket: {}", session.socket_path);
                        println!("#   PID:    {}", session.pid);
                    } else {
                        arf_println!(
                            "IPC server is not running. Use :ipc start or --with-ipc flag."
                        );
                    }
                }
                "send-policy" => match parts.get(2).copied() {
                    Some("allow") => {
                        crate::ipc::set_send_policy_allow(true);
                        arf_println!("IPC send policy: allow");
                    }
                    Some("prompt") => {
                        crate::ipc::set_send_policy_allow(false);
                        arf_println!("IPC send policy: prompt");
                    }
                    _ => arf_println!("Usage: :ipc send-policy prompt|allow"),
                },
                _ => {
                    arf_println!(
                        "Unknown :ipc subcommand. Available: start, stop, status, send-policy"
                    );
                }
            }
            Some(MetaCommandResult::Handled)
        }
        "commands" | "cmds" => {
            arf_println!("Available commands:");
            println!("#   :help          - Search R help");
            println!("#   :info          - Show session information");
            println!("#   :shell         - Enter shell mode (input goes to system shell)");
            println!("#   :r             - Return to R mode (from shell mode)");
            println!("#   :system <cmd>  - Execute a single system command");
            println!("#   :cd <path>     - Change working directory");
            println!("#   :pushd <path>  - Push directory and change to it");
            println!("#   :popd          - Pop directory from stack");
            println!("#   :reprex <on|off|format> - Set reprex mode");
            println!("#   :history       - History management (browse, clear, schema)");
            println!("#   :restart       - Restart R session");
            println!("#   :restart!      - Restart without confirmation");
            println!("#   :switch <ver>  - Restart with different R version (requires rig)");
            println!("#   :switch! <ver> - Switch without confirmation");
            println!(
                "#   :ipc           - IPC server management (start, stop, status, send-policy)"
            );
            println!("#   :changelog     - Show arf changelog");
            println!("#   :commands      - Show this list");
            println!("#   :quit          - Exit arf");
            Some(MetaCommandResult::Handled)
        }
        "quit" | "exit" => Some(MetaCommandResult::Exit),
        "" => {
            // Just ":" with nothing after - show help hint
            arf_println!("Type :commands for available commands");
            Some(MetaCommandResult::Handled)
        }
        _ => Some(MetaCommandResult::Unknown(cmd.to_string())),
    }
}

/// Process :history browse command.
fn process_history_browse(
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    target: &str,
    is_shell_mode: bool,
) -> Option<MetaCommandResult> {
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
            return Some(MetaCommandResult::Handled);
        }
    };

    let Some(store) = runtime.store() else {
        arf_println!("History is unavailable for {} mode.", mode.display_name());
        return Some(MetaCommandResult::Handled);
    };

    Some(MetaCommandResult::ShowHistoryBrowser { store, mode })
}

/// Process :history clear command.
fn process_history_clear(
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    target: &str,
    is_shell_mode: bool,
) -> Option<MetaCommandResult> {
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
            return Some(MetaCommandResult::Handled);
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
        return Some(MetaCommandResult::Handled);
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
        return Some(MetaCommandResult::Handled);
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
        return Some(MetaCommandResult::Handled);
    }

    Some(MetaCommandResult::ClearHistory { stores })
}

fn dedup_history_stores(stores: Vec<(&str, HistoryStore)>) -> Vec<(&str, HistoryStore)> {
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

/// Change the current working directory.
///
/// If `path_arg` is empty, changes to the home directory.
/// Tilde (`~`) is expanded to the home directory.
pub(crate) fn meta_cd(path_arg: &str) -> Result<PathBuf, String> {
    let target = if path_arg.is_empty() {
        dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?
    } else {
        PathBuf::from(expand_tilde(path_arg))
    };
    std::env::set_current_dir(&target).map_err(|e| format!("{}: {}", target.display(), e))?;
    std::env::current_dir().map_err(|e| e.to_string())
}

/// Push the current directory onto the stack and change to a new directory.
///
/// Requires a path argument. Unlike bash's `pushd` (which swaps the top two
/// stack entries when called without arguments), this always requires an
/// explicit destination.
pub(crate) fn meta_pushd(dir_stack: &mut Vec<PathBuf>, path_arg: &str) -> Result<PathBuf, String> {
    if path_arg.is_empty() {
        return Err("Usage: :pushd <path>".to_string());
    }
    let current = std::env::current_dir().map_err(|e| e.to_string())?;
    let new_dir = meta_cd(path_arg)?;
    dir_stack.push(current);
    Ok(new_dir)
}

/// Pop the top directory from the stack and change to it.
pub(crate) fn meta_popd(dir_stack: &mut Vec<PathBuf>) -> Result<PathBuf, String> {
    let target = dir_stack
        .last()
        .cloned()
        .ok_or_else(|| "Directory stack is empty".to_string())?;
    std::env::set_current_dir(&target).map_err(|e| format!("{}", e))?;
    dir_stack.pop();
    std::env::current_dir().map_err(|e| e.to_string())
}

/// Return a hint message if the shell command is a directory navigation command
/// that won't work as expected in a subprocess.
pub(crate) fn dir_command_hint(shell_cmd: &str) -> Option<&'static str> {
    match shell_cmd.split_whitespace().next()? {
        "cd" => Some("Hint: Use the :cd meta command instead to change directory."),
        "pushd" => Some("Hint: Use the :pushd meta command instead to change directory."),
        "popd" => Some("Hint: Use the :popd meta command instead to restore directory."),
        _ => None,
    }
}

#[cfg(test)]
#[path = "meta_command/tests.rs"]
mod tests;
