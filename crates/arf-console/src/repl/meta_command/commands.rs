//! Handlers for the simple meta commands.

use super::directory::{dir_command_hint, meta_cd, meta_popd, meta_pushd};
use super::{MetaCommandContext, MetaCommandResult};
use crate::config::ReprexMode;
use crate::external::formatter;
use crate::meta_command_catalog::{REPREX_MODES, join_choice_names};
use crate::repl::shell::{confirm_action, execute_shell_command};
use crate::repl::{ARF_PREFIX, arf_println};

/// Handle `:reprex on|off|format`.
pub(super) fn handle_reprex(ctx: &mut MetaCommandContext<'_>, parts: &[&str]) -> MetaCommandResult {
    let reprex = &mut *ctx.reprex;
    if parts.len() == 2 {
        match parts[1] {
            "on" => {
                reprex.set_mode(ReprexMode::On);
                arf_println!("Reprex: on");
            }
            "off" => {
                reprex.set_mode(ReprexMode::Off);
                arf_println!("Reprex: off");
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
            }
            _ => print_reprex_usage(),
        }
    } else {
        print_reprex_usage();
    }
    MetaCommandResult::Handled
}

fn print_reprex_usage() {
    arf_println!("Usage: :reprex {}", join_choice_names(REPREX_MODES, "|"));
}

/// Handle `:shell`.
pub(super) fn handle_shell(ctx: &mut MetaCommandContext<'_>) -> MetaCommandResult {
    ctx.prompt_config.set_shell(true);
    arf_println!("Shell mode enabled. Type :r to return to R.");
    MetaCommandResult::Handled
}

/// Handle `:r`.
pub(super) fn handle_r(ctx: &mut MetaCommandContext<'_>) -> MetaCommandResult {
    if ctx.prompt_config.is_shell_enabled() {
        ctx.prompt_config.set_shell(false);
        arf_println!("Returned to R mode.");
    } else {
        arf_println!("Already in R mode.");
    }
    MetaCommandResult::Handled
}

/// Handle `:system <command>`.
///
/// `after_colon` is the input with the leading `:` removed.
pub(super) fn handle_system(after_colon: &str) -> MetaCommandResult {
    // Execute the rest of the input as a shell command
    let shell_cmd = after_colon.strip_prefix("system").unwrap_or("").trim();
    if shell_cmd.is_empty() {
        arf_println!("Usage: :system <command>");
    } else {
        if let Some(hint) = dir_command_hint(shell_cmd) {
            arf_println!("{}", hint);
        }
        execute_shell_command(shell_cmd);
    }
    MetaCommandResult::ShellExecuted
}

/// Handle `:restart` and `:restart!`.
pub(super) fn handle_restart(cmd: &str) -> MetaCommandResult {
    let force = cmd == "restart!";
    if force
        || confirm_action(&format!(
            "{} Restart R session? Current session will be lost.",
            ARF_PREFIX
        ))
    {
        arf_println!("Restarting R session...");
        MetaCommandResult::Restart(None)
    } else {
        arf_println!("Restart cancelled.");
        MetaCommandResult::Handled
    }
}

/// Handle `:switch <version>` and `:switch! <version>`.
///
/// `after_colon` is the input with the leading `:` removed.
pub(super) fn handle_switch(
    ctx: &MetaCommandContext<'_>,
    cmd: &str,
    after_colon: &str,
) -> MetaCommandResult {
    // :switch requires rig to be enabled at startup
    if !ctx.r_source_status.switch_available() {
        arf_println!("Error: :switch requires rig to be available at startup.");
        arf_println!(
            r#"Start arf with r_source = "auto" (with rig installed) or r_source = "rig"."#
        );
        return MetaCommandResult::Handled;
    }

    // Extract the complete version argument, since version ranges may
    // contain spaces (for example, ">=4.3, <5.0").
    let version = after_colon
        .trim_start()
        .strip_prefix(cmd)
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned);
    if version.is_none() {
        arf_println!("Usage: :{cmd} <version>");
        arf_println!("Example: :{cmd} 4.4 or :{cmd} release");
        return MetaCommandResult::Handled;
    }
    let force = cmd == "switch!";
    let ver = version.as_ref().unwrap();
    if force || confirm_action(&format!("Restart with R {}?", ver)) {
        arf_println!("Restarting with R {}...", ver);
        MetaCommandResult::Restart(version)
    } else {
        arf_println!("Switch cancelled.");
        MetaCommandResult::Handled
    }
}

/// Handle `:help [query]`.
pub(super) fn handle_help(parts: &[&str]) -> MetaCommandResult {
    // Fuzzy help search for R documentation
    // Inspired by the felp package: https://github.com/atusy/felp
    let query = parts.get(1..).map(|p| p.join(" ")).unwrap_or_default();
    MetaCommandResult::ShowHelpBrowser(query)
}

/// Handle `:cd [path]`.
///
/// `after_colon` is the input with the leading `:` removed.
pub(super) fn handle_cd(after_colon: &str) -> MetaCommandResult {
    let path_arg = after_colon.strip_prefix("cd").unwrap_or("").trim();
    match meta_cd(path_arg) {
        Ok(cwd) => arf_println!("{}", cwd.display()),
        Err(e) => arf_println!("cd: {}", e),
    }
    MetaCommandResult::Handled
}

/// Handle `:pushd <path>`.
///
/// `after_colon` is the input with the leading `:` removed.
pub(super) fn handle_pushd(
    ctx: &mut MetaCommandContext<'_>,
    after_colon: &str,
) -> MetaCommandResult {
    let path_arg = after_colon.strip_prefix("pushd").unwrap_or("").trim();
    match meta_pushd(ctx.dir_stack, path_arg) {
        Ok(cwd) => arf_println!("{}", cwd.display()),
        Err(e) => arf_println!("pushd: {}", e),
    }
    MetaCommandResult::Handled
}

/// Handle `:popd`.
pub(super) fn handle_popd(ctx: &mut MetaCommandContext<'_>) -> MetaCommandResult {
    match meta_popd(ctx.dir_stack) {
        Ok(cwd) => arf_println!("{}", cwd.display()),
        Err(e) => arf_println!("popd: {}", e),
    }
    MetaCommandResult::Handled
}

/// Handle `:commands`.
pub(super) fn handle_commands() -> MetaCommandResult {
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
    println!("#   :ipc           - IPC server management (start, stop, status, send-policy)");
    println!("#   :changelog     - Show arf changelog");
    println!("#   :commands      - Show this list");
    println!("#   :quit          - Exit arf");
    MetaCommandResult::Handled
}
