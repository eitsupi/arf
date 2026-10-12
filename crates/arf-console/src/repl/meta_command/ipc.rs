//! `:ipc` meta command.

use super::{MetaCommandContext, MetaCommandResult};
use crate::meta_command_catalog::{IPC_SEND_POLICIES, IPC_SUBCOMMANDS, join_choice_names};
use crate::repl::arf_println;

/// Handle `:ipc [start|stop|status|send-policy]`.
pub(super) fn handle_ipc(ctx: &MetaCommandContext<'_>, parts: &[&str]) -> MetaCommandResult {
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
