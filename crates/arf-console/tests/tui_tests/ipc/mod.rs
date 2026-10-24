//! IPC requests made through the real `arf ipc` client against a TUI session.

mod approval;
mod expose;
mod handlers;
mod lifecycle;
mod raw_rpc;
mod silent_visible;

use super::support::Terminal;
use anyhow::{Context, Result};
use std::path::PathBuf;

fn session_metadata_path(terminal: &Terminal) -> Result<PathBuf> {
    Ok(terminal.sessions_dir().join(format!(
        "{}.json",
        terminal.pid().context("missing arf PID")?
    )))
}
