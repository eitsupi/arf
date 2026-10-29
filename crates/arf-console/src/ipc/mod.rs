//! IPC server for external tool access to the arf R session.
//!
//! Provides a JSON-RPC 2.0 interface over Unix sockets (or named pipes on Windows)
//! for AI agents, vscode-R, and other tools to interact with R.
//!
//! ## Mutual exclusion between console and IPC input
//!
//! Interactive operations that could conflict with console input (user_input,
//! visible evaluate, and silent evaluate) are routed through reedline's
//! ExternalBreak mechanism. This allows the REPL to check the editor buffer
//! before accepting or rejecting the operation:
//!
//! 1. Idle callback receives IPC request → stores in `PENDING_IPC_OPERATION` → fires break signal
//! 2. Reedline returns `Signal::ExternalBreak(buffer)` with current editor buffer
//! 3. REPL checks buffer: empty → accept operation, non-empty → reject with `USER_IS_TYPING`
//!
//! Headless operations bypass reedline. At the next top-level `ReadConsole`,
//! the R mainloop takes one queued request and feeds its code from a
//! request-owned input buffer. The request completes when R returns to the
//! next main prompt; nested console reads receive EOF and cannot consume a
//! later IPC request.

mod approval;
pub(crate) mod bridge;
mod capture;
pub mod client;
mod console_output;
pub(crate) use console_output::{
    clear_bus as clear_console_output_bus, flush as flush_console_output,
    install_bus as install_console_output_bus, install_observer as install_console_output_observer,
};
mod events;
mod execution;
pub mod handlers;
mod headless;
mod history_query;
mod pending;
pub mod policy;
pub mod protocol;
pub mod server;
pub mod session;
mod session_result;
mod state;
mod validation;

#[allow(unused_imports)]
pub use approval::{
    UserInputApproval, approve_user_input, reject_user_input_not_approved, send_policy_is_allow,
    set_send_policy_allow,
};
pub(crate) use events::install_r_event_publisher;
pub(crate) use execution::{
    accept_user_input, begin_terminal_execution, current_execution_id, finish_execution,
    run_silent_eval_from,
};
pub use headless::{headless_read_console, set_headless_history};
pub use history_query::set_history_store;
pub(crate) use pending::finish_pending_visible_eval;
pub use pending::{
    PendingIpcKind, poll_ipc_requests, reject_operation_user_typing, setup_visible_eval,
    take_pending_ipc_operation,
};
pub use protocol::ExecutionStatus;
pub use session_result::{clear_history_session_id, read_r_version};
pub use state::{
    break_signal, is_in_alternate_mode, set_headless_shutdown, set_in_alternate_mode,
    set_r_at_prompt,
};

/// Start the IPC server and set up channels.
///
/// Called from `main.rs` when `--with-ipc` is specified,
/// or from `:ipc start` meta command.
///
/// If `bind` is `Some`, the server binds to the given path instead of the
/// default PID-based path.
pub fn start_server(
    bind: Option<&str>,
    r_home: Option<String>,
    r_version: Option<String>,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    session_type: session::SessionType,
) -> std::io::Result<session::SessionInfo> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Initialize pending operation storage
    let _ = pending::pending_ipc_operation();

    // Capture the start time once so both the session file and in-memory
    // cache use the same value.
    let started_at = chrono::Local::now().to_rfc3339();

    // Start the server thread first; only update the receiver after
    // confirming that the server bound successfully, so a failed start
    // doesn't break an already-running server's channel.
    let session = server::start_server(
        tx,
        bind,
        &started_at,
        r_home,
        r_version,
        log_file,
        history_session_id,
        session_type,
    )?;

    // Note: session metadata is now cached inside server::start_server()
    // right after bind confirmation, before the server can serve any request.

    // Store receiver for polling from idle callback.
    state::install_receiver(rx)?;

    Ok(session)
}

/// Stop the IPC server (cleanup on exit).
///
/// Clears any in-flight IPC state (pending operations, active capture)
/// so the REPL doesn't get stuck in a pending/capturing state.
pub fn stop_server() {
    state::clear_receiver();
    pending::cancel_in_flight_operations();
    server::stop_server();
}
