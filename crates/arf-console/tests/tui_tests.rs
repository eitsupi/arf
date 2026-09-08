//! Cross-platform interactive regression tests using tui-test.
//!
//! The ordinary workspace test command runs these cases on Windows, Linux, and
//! macOS. Each case owns a fresh R process, configuration, history and IPC directory.
//! The legacy Unix PTY suite remains a regression baseline during migration.

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
#[path = "tui/session.rs"]
mod session;
#[path = "tui/shell.rs"]
mod shell;
#[path = "tui/support.rs"]
mod support;
