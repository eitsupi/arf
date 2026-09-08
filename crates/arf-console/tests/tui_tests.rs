//! Cross-platform interactive regression tests using tui-test.
//!
//! The ordinary workspace test command runs these cases on Windows, Linux, and
//! macOS. Each case owns a fresh R process, configuration, history and IPC directory.
//! These cases verify interactive screen and prompt behavior; the separate
//! `ipc_tests.rs` integration test keeps low-level JSON-RPC transport coverage.

#[path = "tui/history.rs"]
mod history;
#[path = "tui/input.rs"]
mod input;
#[path = "tui/ipc.rs"]
mod ipc;
#[path = "tui/output.rs"]
mod output;
#[path = "tui/prompt.rs"]
mod prompt;
#[path = "tui/reprex.rs"]
mod reprex;
#[path = "tui/restart.rs"]
mod restart;
#[path = "tui/session.rs"]
mod session;
#[path = "tui/shell.rs"]
mod shell;
#[path = "tui/signal.rs"]
mod signal;
#[path = "tui/support.rs"]
mod support;
#[path = "tui/ui.rs"]
mod ui;
