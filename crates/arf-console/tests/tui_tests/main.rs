//! Cross-platform interactive regression tests using tui-test.
//!
//! The ordinary workspace test command runs these cases on Windows, Linux, and
//! macOS. Each case owns a fresh R process, configuration, history and IPC directory.
//! These cases verify interactive screen and prompt behavior, plus the raw
//! JSON-RPC transport shape in the `ipc` module.

#[path = "../common/mod.rs"]
mod common;
mod config;
mod history;
mod history_status;
mod input;
mod ipc;
mod output;
mod prompt;
mod reprex;
mod restart;
mod session;
mod shell;
mod signal;
mod support;
mod ui;
