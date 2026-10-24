//! `:ipc` meta command.

use super::{MetaCommandContext, MetaCommandResult};
use crate::meta_command_catalog::{IPC_SEND_POLICIES, IPC_SUBCOMMANDS, join_choice_names};
use crate::repl::arf_println;

/// Handle `:ipc [start|stop|status|revoke|send-policy|expose]`.
pub(super) fn handle_ipc(
    ctx: &MetaCommandContext<'_>,
    parts: &[&str],
    input: &str,
    allow_expose: bool,
) -> MetaCommandResult {
    let subcmd = parts.get(1).copied().unwrap_or("status");
    match subcmd {
        "start" => match crate::ipc::start_server(
            None,
            ctx.r_home.map(|path| path.display().to_string()),
            crate::ipc::read_r_version(),
            None,
            ctx.history_session_id,
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
                arf_println!("IPC server is not running. Use :ipc start or --with-ipc flag.");
            }
            let status = crate::ipc::handlers::handler_status();
            arf_println!("IPC handlers: {:?}", status.state);
            for name in status.handlers {
                arf_println!("  {}", name.escape_debug());
            }
        }
        "revoke" => {
            crate::ipc::handlers::revoke_handlers();
            arf_println!("IPC handlers revoked for this process.");
        }
        "expose" => {
            let Some(expression) = expose_expression(input) else {
                arf_println!("Usage: :ipc expose <R expression>");
                return MetaCommandResult::Handled;
            };
            if !allow_expose {
                arf_println!(
                    "IPC handler exposure is available only at the interactive R command prompt."
                );
                return MetaCommandResult::Handled;
            }
            expose_handlers(expression);
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
            _ => arf_println!(
                "Usage: :ipc send-policy {}",
                join_choice_names(IPC_SEND_POLICIES, "|")
            ),
        },
        _ => {
            arf_println!(
                "Unknown :ipc subcommand. Available: {}",
                join_choice_names(IPC_SUBCOMMANDS, ", ")
            );
        }
    }
    MetaCommandResult::Handled
}

/// Extract the expression after `:ipc expose`, retaining its original
/// whitespace and line breaks after the command separator.
fn expose_expression(input: &str) -> Option<&str> {
    let input = input.trim_start();
    let input = input.strip_prefix(':')?.trim_start();
    let input = input.strip_prefix("ipc")?;
    if !input.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let input = input.trim_start();
    let input = input.strip_prefix("expose")?;
    if !input.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let expression = input.trim_start();
    (!expression.is_empty()).then_some(expression)
}

/// Evaluate, preview, and independently approve a candidate handler registry.
fn expose_handlers(expression: &str) {
    match arf_libr::frame_depth_protected() {
        Ok(0) => {}
        Ok(_) => {
            arf_println!("IPC handlers can only be exposed at the top-level R prompt.");
            return;
        }
        Err(error) => {
            arf_println!("Could not verify the R evaluation depth: {error}");
            return;
        }
    }

    // Keep IPC dispatch from starting R work while candidate preparation or
    // the user's independent confirmation is in progress. Do not process R
    // events here: ReadConsole may be suppressing SIGINT and nested event
    // processing could re-enter R while its frame stack is being inspected.
    let _guard = IpcExposeGuard::new();
    let prepare_result = {
        // SAFETY: this runs on ReadConsole's R thread, while prepare_handlers
        // keeps all R work inside R_ToplevelExec; the guard outlives that call.
        let _interrupt_guard = unsafe { arf_libr::ConsoleEvaluationInterruptGuard::new() };
        crate::ipc::handlers::prepare_handlers(expression)
    };
    // This meta command does not enter the normal command lifecycle. Keep a
    // caught error in its expression from marking the next R command failed.
    arf_libr::reset_command_error_state();
    let prepared = match prepare_result {
        Ok(prepared) => prepared,
        Err(error) => {
            arf_println!("Failed to prepare IPC handlers: {error}");
            return;
        }
    };

    arf_println!(
        "The R expression has already been evaluated; its side effects cannot be undone by declining."
    );
    arf_println!("Candidate IPC handlers:");
    for name in prepared.names() {
        arf_println!("  {}", name.escape_debug());
    }
    if !confirm_handler_exposure() {
        arf_println!("IPC handler exposure declined.");
        return;
    }
    match crate::ipc::handlers::publish_handlers(prepared) {
        Ok(()) => arf_println!("IPC handlers exposed."),
        Err(error) => arf_println!("Failed to expose IPC handlers: {error}"),
    }
}

struct IpcExposeGuard {
    was_alternate: bool,
}

impl IpcExposeGuard {
    fn new() -> Self {
        let was_alternate = crate::ipc::is_in_alternate_mode();
        crate::ipc::set_r_at_prompt(false);
        crate::ipc::set_in_alternate_mode(true);
        Self { was_alternate }
    }
}

impl Drop for IpcExposeGuard {
    fn drop(&mut self) {
        crate::ipc::set_in_alternate_mode(self.was_alternate);
        crate::ipc::set_r_at_prompt(true);
    }
}

/// Ask for explicit approval regardless of the interactive send policy.
/// Ctrl+C cancels the candidate; only y/Y publishes it.
fn confirm_handler_exposure() -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use std::io::Write;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw && let Err(error) = crossterm::terminal::enable_raw_mode() {
        arf_println!("Could not enable terminal input for confirmation: {error}");
        return false;
    }
    struct RawModeGuard(bool);
    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            if !self.0 {
                let _ = crossterm::terminal::disable_raw_mode();
            }
        }
    }
    let _raw_mode = RawModeGuard(was_raw);
    print!("Publish these IPC handlers? [y/N] ");
    let _ = std::io::stdout().flush();

    loop {
        match event::poll(std::time::Duration::from_millis(50)) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => {
                arf_println!("\r\nIPC handler confirmation failed: {error}");
                return false;
            }
        }
        let event = match event::read() {
            Ok(event) => event,
            Err(error) => {
                arf_println!("\r\nIPC handler confirmation failed: {error}");
                return false;
            }
        };
        let Event::Key(key) = event else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        let approved = matches!(key.code, KeyCode::Char('y' | 'Y'))
            && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT);
        while event::poll(std::time::Duration::ZERO).unwrap_or(false) {
            if event::read().is_err() {
                break;
            }
        }
        if ctrl_c {
            print!("\r\n^C\r\n");
            let _ = std::io::stdout().flush();
            return false;
        }
        print!("\r\n");
        let _ = std::io::stdout().flush();
        return approved;
    }
}
