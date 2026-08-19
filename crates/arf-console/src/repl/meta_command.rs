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
                if reprex.mode != ReprexMode::Format
                    && !formatter::is_formatter_available(reprex.formatter)
                {
                    arf_println!(
                        "{}",
                        formatter::unavailable_message(
                            reprex.formatter,
                            formatter::FormatterUnavailableContext::MetaCommand
                        )
                    );
                } else {
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
mod tests {
    use super::*;
    use crate::editor::prompt::PromptFormatter;
    use crate::history::HistoryFailureDetail;
    use crate::repl::reprex::ReprexRuntime;

    fn create_test_prompt_config() -> PromptRuntimeConfig {
        PromptRuntimeConfig::builder(PromptFormatter::default(), "r> ", "+  ", "[bash] $ ").build()
    }

    /// Default r_source_status for tests (PATH mode, rig not enabled).
    fn default_r_source_status() -> RSourceStatus {
        RSourceStatus::Path
    }

    /// Helper to call process_meta_command with default dir_stack.
    fn call_meta(
        input: &str,
        config: &mut PromptRuntimeConfig,
        _r_history_path: &Option<PathBuf>,
        _shell_history_path: &Option<PathBuf>,
        status: &RSourceStatus,
    ) -> Option<MetaCommandResult> {
        let mut dir_stack = Vec::new();
        let mut reprex =
            ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::ReprexFormatter::Air);
        process_meta_command(
            input,
            config,
            &mut reprex,
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            status,
            &mut dir_stack,
            None,
            None,
        )
    }

    fn call_meta_with_runtime(
        input: &str,
        config: &mut PromptRuntimeConfig,
        reprex: &mut ReprexRuntime,
        status: &RSourceStatus,
    ) -> Option<MetaCommandResult> {
        let mut dir_stack = Vec::new();
        process_meta_command(
            input,
            config,
            reprex,
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            status,
            &mut dir_stack,
            None,
            None,
        )
    }

    #[test]
    fn test_process_meta_command_not_meta() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta("print(x)", &mut config, &None, &None, &status);
        assert!(result.is_none());
    }

    #[test]
    fn test_process_meta_command_reprex_explicit_mode() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let mut reprex =
            ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::ReprexFormatter::Air);
        let status = default_r_source_status();
        assert!(!reprex.is_enabled());

        let result = call_meta_with_runtime(":reprex on", &mut config, &mut reprex, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(reprex.is_enabled());

        let result = call_meta_with_runtime(":reprex off", &mut config, &mut reprex, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!reprex.is_enabled());

        let result = call_meta_with_runtime(":reprex", &mut config, &mut reprex, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!reprex.is_enabled());

        // Once already in format mode, repeating the command is idempotent
        // and does not require probing the formatter again.
        reprex.set_mode(ReprexMode::Format);
        let result = call_meta_with_runtime(":reprex format", &mut config, &mut reprex, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert_eq!(reprex.mode, ReprexMode::Format);

        let result = call_meta_with_runtime(":reprex on extra", &mut config, &mut reprex, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(reprex.is_enabled());
    }

    #[test]
    fn test_process_meta_command_commands() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":commands", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));

        // Test alias
        let result = call_meta(":cmds", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    fn test_process_meta_command_info() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":info", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::ShowSessionInfo)));

        // Test alias
        let result = call_meta(":session", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::ShowSessionInfo)));
    }

    #[test]
    fn test_process_meta_command_quit() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":quit", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Exit)));

        let result = call_meta(":exit", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Exit)));
    }

    #[test]
    fn test_process_meta_command_unknown() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":unknown", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Unknown(_))));
    }

    #[test]
    fn test_process_meta_command_empty_colon() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    fn test_process_meta_command_with_whitespace() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta("  :reprex  ", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        // Bare commands do not change the independent runtime state.
    }

    #[test]
    fn test_process_meta_command_shell_enter() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        assert!(!config.is_shell_enabled());

        let result = call_meta(":shell", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(config.is_shell_enabled());
    }

    #[test]
    fn test_process_meta_command_shell_exit_with_r() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        config.set_shell(true);
        assert!(config.is_shell_enabled());

        let result = call_meta(":r", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!config.is_shell_enabled());
    }

    #[test]
    fn test_process_meta_command_shell_exit_with_uppercase_r() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        config.set_shell(true);
        assert!(config.is_shell_enabled());

        let result = call_meta(":R", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!config.is_shell_enabled());
    }

    #[test]
    fn test_process_meta_command_r_when_not_in_shell() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        assert!(!config.is_shell_enabled());

        let result = call_meta(":r", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!config.is_shell_enabled()); // Still not in shell
    }

    #[test]
    fn test_process_meta_command_system() {
        // create_test_prompt_config and execute_shell_command read SHELL indirectly.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":system echo hello", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::ShellExecuted)));
    }

    #[test]
    fn test_process_meta_command_system_empty() {
        // create_test_prompt_config and execute_shell_command read SHELL indirectly.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":system", &mut config, &None, &None, &status);
        // Empty :system should still be handled
        assert!(matches!(result, Some(MetaCommandResult::ShellExecuted)));
    }

    #[test]
    fn test_process_meta_command_switch_requires_rig() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();

        // With PATH mode (rig not enabled), :switch should show error
        let status_path = RSourceStatus::Path;
        let result = call_meta(":switch 4.4", &mut config, &None, &None, &status_path);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));

        // With Rig mode (rig enabled), :switch should work (but needs confirmation which we can't test here)
        // Just verify it doesn't immediately reject
        let status_rig = RSourceStatus::Rig {
            version: "4.4.0".to_string(),
            override_info: None,
        };
        // Note: This will prompt for confirmation, so we can't fully test it in unit tests
        // Just testing the setup path here
        let result = call_meta(":switch", &mut config, &None, &None, &status_rig);
        // Without version argument, it should show usage
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    // --- cd/pushd/popd tests ---

    #[test]
    fn test_meta_cd_relative_path() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("sub");
        std::fs::create_dir(&subdir).unwrap();

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = meta_cd("sub");

        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("sub"));
    }

    #[test]
    fn test_meta_cd_absolute_path() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();

        let result = meta_cd(&tmp.path().to_string_lossy());

        assert!(result.is_ok());
    }

    #[test]
    fn test_meta_cd_tilde() {
        // meta_cd reads HOME through dirs::home_dir and changes the cwd.
        let _guard = crate::test_utils::lock_env_and_cwd();
        let result = meta_cd("~");

        assert!(result.is_ok());
        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                result.unwrap().canonicalize().ok(),
                home.canonicalize().ok()
            );
        }
    }

    #[test]
    fn test_meta_cd_no_args() {
        // meta_cd reads HOME through dirs::home_dir and changes the cwd.
        let _guard = crate::test_utils::lock_env_and_cwd();
        let result = meta_cd("");

        assert!(result.is_ok());
        // Should go to home
        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                result.unwrap().canonicalize().ok(),
                home.canonicalize().ok()
            );
        }
    }

    #[test]
    fn test_meta_cd_nonexistent() {
        let result = meta_cd("/nonexistent_path_12345");
        assert!(result.is_err());
    }

    #[test]
    fn test_meta_pushd_popd() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let mut dir_stack = Vec::new();

        std::env::set_current_dir(tmp.path()).unwrap();
        let subdir = tmp.path().join("sub");
        std::fs::create_dir(&subdir).unwrap();

        // pushd into sub
        let result = meta_pushd(&mut dir_stack, "sub");
        assert!(result.is_ok());
        assert_eq!(dir_stack.len(), 1);
        assert!(std::env::current_dir().unwrap().ends_with("sub"));

        // popd back
        let result = meta_popd(&mut dir_stack);
        assert!(result.is_ok());
        assert!(dir_stack.is_empty());
        assert_eq!(
            std::env::current_dir().unwrap().canonicalize().ok(),
            tmp.path().canonicalize().ok()
        );
    }

    #[test]
    fn test_meta_popd_empty_stack() {
        let mut dir_stack: Vec<PathBuf> = Vec::new();
        let result = meta_popd(&mut dir_stack);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_meta_pushd_no_args_returns_error() {
        let mut dir_stack: Vec<PathBuf> = Vec::new();
        let result = meta_pushd(&mut dir_stack, "");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Usage"));
        // Stack should not be modified
        assert!(dir_stack.is_empty());
    }

    #[test]
    fn test_meta_pushd_saves_previous() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("sub");
        std::fs::create_dir(&subdir).unwrap();
        let mut dir_stack = Vec::new();

        std::env::set_current_dir(tmp.path()).unwrap();
        let before_pushd = std::env::current_dir().unwrap();

        let _ = meta_pushd(&mut dir_stack, "sub");
        assert_eq!(dir_stack.len(), 1);
        assert_eq!(
            dir_stack[0].canonicalize().ok(),
            before_pushd.canonicalize().ok()
        );
    }

    #[test]
    fn test_process_meta_command_cd() {
        // create_test_prompt_config reads SHELL; this test also changes the cwd.
        let _guard = crate::test_utils::lock_env_and_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let mut dir_stack = Vec::new();

        let result = process_meta_command(
            &format!(":cd {}", tmp.path().display()),
            &mut config,
            &mut ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::ReprexFormatter::Air),
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &status,
            &mut dir_stack,
            None,
            None,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    fn test_process_meta_command_pushd_popd() {
        // create_test_prompt_config reads SHELL; this test also changes the cwd.
        let _guard = crate::test_utils::lock_env_and_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let mut dir_stack = Vec::new();

        let result = process_meta_command(
            &format!(":pushd {}", tmp.path().display()),
            &mut config,
            &mut ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::ReprexFormatter::Air),
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &status,
            &mut dir_stack,
            None,
            None,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert_eq!(dir_stack.len(), 1);

        let result = process_meta_command(
            ":popd",
            &mut config,
            &mut ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::ReprexFormatter::Air),
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &HistoryRuntime::Unavailable {
                failure: HistoryFailureDetail::test_memory(),
                previous_failure: None,
            },
            &status,
            &mut dir_stack,
            None,
            None,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(dir_stack.is_empty());
    }

    // --- dir_command_hint tests ---

    #[test]
    fn test_dir_command_hint_cd() {
        assert!(dir_command_hint("cd /tmp").unwrap().contains(":cd"));
    }

    #[test]
    fn test_dir_command_hint_pushd() {
        assert!(dir_command_hint("pushd /tmp").unwrap().contains(":pushd"));
    }

    #[test]
    fn test_dir_command_hint_popd() {
        assert!(dir_command_hint("popd").unwrap().contains(":popd"));
    }

    #[test]
    fn test_dir_command_hint_other() {
        assert!(dir_command_hint("ls -la").is_none());
        assert!(dir_command_hint("echo cd").is_none());
    }

    #[test]
    fn test_dir_command_hint_empty() {
        assert!(dir_command_hint("").is_none());
    }

    // --- Force (!) option tests ---

    #[test]
    fn test_process_meta_command_restart_force() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        // :restart! should skip confirmation and return Restart directly
        let result = call_meta(":restart!", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Restart(None))));
    }

    #[test]
    fn test_process_meta_command_restart_force_with_whitespace() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta("  :restart!  ", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Restart(None))));
    }

    #[test]
    fn test_process_meta_command_switch_force() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status_rig = RSourceStatus::Rig {
            version: "4.4.0".to_string(),
            override_info: None,
        };
        let result = call_meta(":switch! 4.4", &mut config, &None, &None, &status_rig);
        assert!(matches!(result, Some(MetaCommandResult::Restart(Some(ref v))) if v == "4.4"));
    }

    #[test]
    fn test_process_meta_command_switch_force_accepts_space_after_colon() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status_rig = RSourceStatus::Rig {
            version: "4.4.0".to_string(),
            override_info: None,
        };

        let result = call_meta(": switch! 4.4", &mut config, &None, &None, &status_rig);

        assert!(matches!(
            result,
            Some(MetaCommandResult::Restart(Some(ref version))) if version == "4.4"
        ));
    }

    #[test]
    fn test_process_meta_command_switch_force_accepts_spaced_version_range() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status_rig = RSourceStatus::Rig {
            version: "4.4.0".to_string(),
            override_info: None,
        };

        let result = call_meta(
            ":switch! >=4.3, <5.0",
            &mut config,
            &None,
            &None,
            &status_rig,
        );

        assert!(matches!(
            result,
            Some(MetaCommandResult::Restart(Some(ref version)))
                if version == ">=4.3, <5.0"
        ));
    }

    #[test]
    fn test_process_meta_command_switch_force_preserves_named_version() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status_rig = RSourceStatus::Rig {
            version: "4.4.0".to_string(),
            override_info: None,
        };

        let result = call_meta(":switch! release", &mut config, &None, &None, &status_rig);

        assert!(matches!(
            result,
            Some(MetaCommandResult::Restart(Some(ref version))) if version == "release"
        ));
    }

    #[test]
    fn test_process_meta_command_switch_force_no_version() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status_rig = RSourceStatus::Rig {
            version: "4.4.0".to_string(),
            override_info: None,
        };
        // :switch! without version should still show usage
        let result = call_meta(":switch!", &mut config, &None, &None, &status_rig);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    fn test_process_meta_command_unknown_with_bang() {
        // create_test_prompt_config reads SHELL through PromptFormatter::new.
        let _guard = crate::test_utils::lock_env();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        // :shell! is not a valid command (! only supported on restart/switch)
        let result = call_meta(":shell!", &mut config, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Unknown(_))));
    }

    #[test]
    fn history_clear_deduplicates_shared_store_owners() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path().join("history.db"), None, None).unwrap();
        let unique = HistoryStore::open(dir.path().join("other.db"), None, None).unwrap();
        let stores =
            dedup_history_stores(vec![("R", store.clone()), ("Shell", store), ("R", unique)]);
        assert_eq!(stores.len(), 2);
        assert_eq!(stores[0].0, "R");
    }
}
