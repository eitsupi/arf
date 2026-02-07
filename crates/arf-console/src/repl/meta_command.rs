//! Meta command processing.

use crate::completion::path::expand_tilde;
use crate::config::RSourceStatus;
use crate::external::formatter;
use crate::pager::{
    HistoryBrowserResult, HistoryDbMode, display_session_info, run_help_browser,
    run_history_browser, text_utils,
};
use reedline::{History, SqliteBackedHistory};
use std::path::PathBuf;

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
}

/// Process a meta command (starting with `:`) and return the result.
pub fn process_meta_command(
    input: &str,
    prompt_config: &mut PromptRuntimeConfig,
    config_path: &Option<PathBuf>,
    r_history_path: &Option<PathBuf>,
    shell_history_path: &Option<PathBuf>,
    r_source_status: &RSourceStatus,
    dir_stack: &mut Vec<PathBuf>,
) -> Option<MetaCommandResult> {
    let trimmed = input.trim();
    if !trimmed.starts_with(':') {
        return None;
    }

    let parts: Vec<&str> = trimmed[1..].split_whitespace().collect();
    let cmd = parts.first().copied().unwrap_or("");

    match cmd {
        "reprex" => {
            prompt_config.toggle_reprex();
            if prompt_config.is_reprex_enabled() {
                if prompt_config.is_autoformat_enabled() {
                    println!(
                        "# Reprex mode enabled (comment: {:?}, auto-format: on)",
                        prompt_config.reprex_comment
                    );
                } else {
                    arf_println!(
                        "Reprex mode enabled (comment: {:?})",
                        prompt_config.reprex_comment
                    );
                }
            } else {
                arf_println!("Reprex mode disabled");
            }
            Some(MetaCommandResult::Handled)
        }
        "autoformat" | "format" => {
            if prompt_config.is_autoformat_enabled() {
                // Disabling - always allowed
                prompt_config.toggle_autoformat();
                arf_println!("Autoformat disabled");
            } else {
                // Enabling - check if Air is available
                if formatter::is_formatter_available() {
                    prompt_config.toggle_autoformat();
                    if prompt_config.is_reprex_enabled() {
                        arf_println!("Autoformat enabled");
                    } else {
                        arf_println!("Autoformat enabled (activate reprex mode to use)");
                    }
                } else {
                    arf_println!(
                        "Error: Cannot enable autoformat - Air CLI ('air' command) not found in PATH."
                    );
                    arf_println!("Install Air CLI from https://github.com/posit-dev/air");
                }
            }
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
                super::shell::execute_shell_command(shell_cmd);
            }
            Some(MetaCommandResult::ShellExecuted)
        }
        "restart" => {
            if confirm_action(&format!(
                "{} Restart R session? Current session will be lost.",
                ARF_PREFIX
            )) {
                arf_println!("Restarting R session...");
                Some(MetaCommandResult::Restart(None))
            } else {
                arf_println!("Restart cancelled.");
                Some(MetaCommandResult::Handled)
            }
        }
        "switch" => {
            // :switch requires rig to be enabled at startup
            if !r_source_status.rig_enabled() {
                arf_println!("Error: :switch requires rig to be available at startup.");
                arf_println!(
                    r#"Start arf with r_source = "auto" (with rig installed) or r_source = "rig"."#
                );
                return Some(MetaCommandResult::Handled);
            }

            // Extract version argument
            let version = parts.get(1).map(|s| s.to_string());
            if version.is_none() {
                arf_println!("Usage: :switch <version>");
                arf_println!("Example: :switch 4.4 or :switch release");
                return Some(MetaCommandResult::Handled);
            }
            let ver = version.as_ref().unwrap();
            if confirm_action(&format!("Restart with R {}?", ver)) {
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
                        r_history_path,
                        shell_history_path,
                        target,
                        prompt_config.is_shell_enabled(),
                    )
                }
                "clear" => {
                    let target = parts.get(2).copied().unwrap_or("");
                    process_history_clear(
                        r_history_path,
                        shell_history_path,
                        target,
                        prompt_config.is_shell_enabled(),
                    )
                }
                "schema" => {
                    if let Err(e) = crate::pager::history_schema::show_schema_pager() {
                        arf_println!("Error: {}", e);
                    }
                    Some(MetaCommandResult::Handled)
                }
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
            if let Err(e) = run_help_browser(&query) {
                arf_println!("Error in help browser: {}", e);
            }
            Some(MetaCommandResult::Handled)
        }
        "info" | "session" => {
            display_session_info(
                prompt_config,
                config_path,
                r_history_path,
                shell_history_path,
                r_source_status,
            );
            Some(MetaCommandResult::Handled)
        }
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
            println!("#   :reprex        - Toggle reprex mode");
            println!(
                "#   :autoformat    - Toggle auto-formatting in reprex mode (requires Air CLI)"
            );
            println!("#   :history       - History management (browse, clear, schema)");
            println!("#   :restart       - Restart R session");
            println!("#   :switch <ver>  - Restart with different R version (requires rig)");
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
    r_history_path: &Option<PathBuf>,
    shell_history_path: &Option<PathBuf>,
    target: &str,
    is_shell_mode: bool,
) -> Option<MetaCommandResult> {
    // Determine which database to browse
    let (mode, path) = match target {
        "" => {
            // Default: browse based on current mode
            if is_shell_mode {
                (HistoryDbMode::Shell, shell_history_path.as_ref())
            } else {
                (HistoryDbMode::R, r_history_path.as_ref())
            }
        }
        "r" | "R" => (HistoryDbMode::R, r_history_path.as_ref()),
        "shell" => (HistoryDbMode::Shell, shell_history_path.as_ref()),
        _ => {
            arf_println!("Unknown target: {}. Use r or shell.", target);
            return Some(MetaCommandResult::Handled);
        }
    };

    let Some(db_path) = path else {
        arf_println!("History is disabled for {} mode.", mode.display_name());
        return Some(MetaCommandResult::Handled);
    };

    match run_history_browser(db_path, mode) {
        Ok(HistoryBrowserResult::Copied(cmd)) => {
            // Truncate long commands for display (display-width aware)
            let display = text_utils::truncate_to_width(&cmd, 60);
            arf_println!("Copied: {}", display);
            Some(MetaCommandResult::Handled)
        }
        Ok(HistoryBrowserResult::Cancelled) => Some(MetaCommandResult::Handled),
        Err(e) => {
            arf_println!("Error: {}", e);
            Some(MetaCommandResult::Handled)
        }
    }
}

/// Process :history clear command.
fn process_history_clear(
    r_history_path: &Option<PathBuf>,
    shell_history_path: &Option<PathBuf>,
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
    let paths_to_clear: Vec<(&str, &PathBuf)> = match clear_target {
        "r" => r_history_path
            .as_ref()
            .map(|p| vec![("R", p)])
            .unwrap_or_default(),
        "shell" => shell_history_path
            .as_ref()
            .map(|p| vec![("Shell", p)])
            .unwrap_or_default(),
        "all" => {
            let mut paths = Vec::new();
            if let Some(p) = r_history_path.as_ref() {
                paths.push(("R", p));
            }
            if let Some(p) = shell_history_path.as_ref() {
                paths.push(("Shell", p));
            }
            paths
        }
        _ => unreachable!(),
    };

    if paths_to_clear.is_empty() {
        arf_println!("History is disabled.");
        return Some(MetaCommandResult::Handled);
    }

    // Count total entries across all targeted databases
    let mut total_count = 0i64;
    let mut counts: Vec<(&str, i64)> = Vec::new();

    for (name, path) in &paths_to_clear {
        match SqliteBackedHistory::with_file((*path).clone(), None, None) {
            Ok(history) => {
                if let Ok(count) = history.count_all() {
                    counts.push((name, count));
                    total_count += count;
                }
            }
            Err(_) => {
                // Database doesn't exist yet, treat as 0 entries
                counts.push((name, 0));
            }
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

    // Perform clear on each database
    let mut cleared_count = 0i64;
    for (name, path) in &paths_to_clear {
        match SqliteBackedHistory::with_file((*path).clone(), None, None) {
            Ok(mut history) => {
                if let Ok(count) = history.count_all()
                    && count > 0
                {
                    if let Err(e) = history.clear() {
                        arf_println!("Failed to clear {} history: {}", name, e);
                    } else {
                        cleared_count += count;
                    }
                }
            }
            Err(_) => {
                // Database doesn't exist, nothing to clear
            }
        }
    }

    arf_println!("Cleared {} history entries.", cleared_count);
    Some(MetaCommandResult::Handled)
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
    dir_stack.push(current);
    meta_cd(path_arg)
}

/// Pop the top directory from the stack and change to it.
pub(crate) fn meta_popd(dir_stack: &mut Vec<PathBuf>) -> Result<PathBuf, String> {
    let target = dir_stack
        .pop()
        .ok_or_else(|| "Directory stack is empty".to_string())?;
    std::env::set_current_dir(&target).map_err(|e| format!("{}", e))?;
    std::env::current_dir().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::prompt::PromptFormatter;

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
        config_path: &Option<PathBuf>,
        r_history_path: &Option<PathBuf>,
        shell_history_path: &Option<PathBuf>,
        status: &RSourceStatus,
    ) -> Option<MetaCommandResult> {
        let mut dir_stack = Vec::new();
        process_meta_command(
            input,
            config,
            config_path,
            r_history_path,
            shell_history_path,
            status,
            &mut dir_stack,
        )
    }

    #[test]
    fn test_process_meta_command_not_meta() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta("print(x)", &mut config, &None, &None, &None, &status);
        assert!(result.is_none());
    }

    #[test]
    fn test_process_meta_command_reprex_toggle() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        assert!(!config.is_reprex_enabled());

        let result = call_meta(":reprex", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(config.is_reprex_enabled());

        let result = call_meta(":reprex", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!config.is_reprex_enabled());
    }

    #[test]
    fn test_process_meta_command_commands() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":commands", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));

        // Test alias
        let result = call_meta(":cmds", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    #[cfg_attr(windows, ignore)] // Windows CI lacks terminal for interactive pager
    fn test_process_meta_command_info() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":info", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));

        // Test alias
        let result = call_meta(":session", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    #[cfg_attr(windows, ignore)] // Windows CI lacks terminal for interactive pager
    fn test_process_meta_command_info_with_config_path() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();

        // Test with existing config path (using tempfile)
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let existing_path = temp_file.path().to_path_buf();
        let result = call_meta(
            ":info",
            &mut config,
            &Some(existing_path),
            &None,
            &None,
            &status,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));

        // Test with non-existing config path (using tempfile directory with fake filename)
        let temp_dir = tempfile::tempdir().unwrap();
        let non_existing_path = temp_dir.path().join("nonexistent_config.toml");
        let result = call_meta(
            ":info",
            &mut config,
            &Some(non_existing_path),
            &None,
            &None,
            &status,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));

        // Test with None config path (using defaults)
        let result = call_meta(":info", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    fn test_process_meta_command_quit() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":quit", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Exit)));

        let result = call_meta(":exit", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Exit)));
    }

    #[test]
    fn test_process_meta_command_unknown() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":unknown", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Unknown(_))));
    }

    #[test]
    fn test_process_meta_command_empty_colon() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    fn test_process_meta_command_with_whitespace() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta("  :reprex  ", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(config.is_reprex_enabled());
    }

    #[test]
    fn test_process_meta_command_shell_enter() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        assert!(!config.is_shell_enabled());

        let result = call_meta(":shell", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(config.is_shell_enabled());
    }

    #[test]
    fn test_process_meta_command_shell_exit_with_r() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        config.set_shell(true);
        assert!(config.is_shell_enabled());

        let result = call_meta(":r", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!config.is_shell_enabled());
    }

    #[test]
    fn test_process_meta_command_shell_exit_with_uppercase_r() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        config.set_shell(true);
        assert!(config.is_shell_enabled());

        let result = call_meta(":R", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!config.is_shell_enabled());
    }

    #[test]
    fn test_process_meta_command_r_when_not_in_shell() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        assert!(!config.is_shell_enabled());

        let result = call_meta(":r", &mut config, &None, &None, &None, &status);
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(!config.is_shell_enabled()); // Still not in shell
    }

    #[test]
    fn test_process_meta_command_system() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(
            ":system echo hello",
            &mut config,
            &None,
            &None,
            &None,
            &status,
        );
        assert!(matches!(result, Some(MetaCommandResult::ShellExecuted)));
    }

    #[test]
    fn test_process_meta_command_system_empty() {
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let result = call_meta(":system", &mut config, &None, &None, &None, &status);
        // Empty :system should still be handled
        assert!(matches!(result, Some(MetaCommandResult::ShellExecuted)));
    }

    #[test]
    fn test_process_meta_command_switch_requires_rig() {
        let mut config = create_test_prompt_config();

        // With PATH mode (rig not enabled), :switch should show error
        let status_path = RSourceStatus::Path;
        let result = call_meta(
            ":switch 4.4",
            &mut config,
            &None,
            &None,
            &None,
            &status_path,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));

        // With Rig mode (rig enabled), :switch should work (but needs confirmation which we can't test here)
        // Just verify it doesn't immediately reject
        let status_rig = RSourceStatus::Rig {
            version: "4.4.0".to_string(),
        };
        // Note: This will prompt for confirmation, so we can't fully test it in unit tests
        // Just testing the setup path here
        let result = call_meta(":switch", &mut config, &None, &None, &None, &status_rig);
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
        let _guard = crate::test_utils::lock_cwd();
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
        let _guard = crate::test_utils::lock_cwd();
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
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let mut dir_stack = Vec::new();

        let result = process_meta_command(
            &format!(":cd {}", tmp.path().display()),
            &mut config,
            &None,
            &None,
            &None,
            &status,
            &mut dir_stack,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
    }

    #[test]
    fn test_process_meta_command_pushd_popd() {
        let _guard = crate::test_utils::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        let mut config = create_test_prompt_config();
        let status = default_r_source_status();
        let mut dir_stack = Vec::new();

        let result = process_meta_command(
            &format!(":pushd {}", tmp.path().display()),
            &mut config,
            &None,
            &None,
            &None,
            &status,
            &mut dir_stack,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert_eq!(dir_stack.len(), 1);

        let result = process_meta_command(
            ":popd",
            &mut config,
            &None,
            &None,
            &None,
            &status,
            &mut dir_stack,
        );
        assert!(matches!(result, Some(MetaCommandResult::Handled)));
        assert!(dir_stack.is_empty());
    }
}
