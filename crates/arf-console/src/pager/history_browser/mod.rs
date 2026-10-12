//! Interactive history browser for viewing and managing command history.
//!
//! This module provides a terminal-based browser for viewing, filtering,
//! and batch-deleting command history entries stored in SQLite.

mod events;
mod filter;
mod render;
mod state;

use crate::history::HistoryStore;
use reedline::HistoryItem;
use state::HistoryBrowser;
use std::io;

/// Maximum number of history entries to load from database.
const MAX_ENTRIES: i64 = 10000;

/// Database mode for history browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryDbMode {
    /// R command history.
    R,
    /// Shell command history.
    Shell,
}

impl HistoryDbMode {
    /// Display name for the mode.
    pub fn display_name(&self) -> &'static str {
        match self {
            HistoryDbMode::R => "R",
            HistoryDbMode::Shell => "Shell",
        }
    }
}

/// Result of running the history browser.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants provide useful API even if not all fields are read
pub enum HistoryBrowserResult {
    /// User exited without action.
    Cancelled,
    /// User copied a command to clipboard.
    Copied(String),
}

/// Load history entries through the already-open arf-owned store.
///
/// The browser keeps no database lock while its UI loop is running.
fn load_history(store: &HistoryStore) -> io::Result<Vec<HistoryItem>> {
    let mut query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    query.limit = Some(MAX_ENTRIES);
    store.search(query).map_err(io::Error::other)
}

/// Run the history browser.
///
/// # Arguments
/// * `store` - The already-open arf-owned history store
/// * `mode` - The database mode (R or Shell)
///
/// # Returns
/// The result of the browser interaction.
pub fn run_history_browser(
    store: &HistoryStore,
    mode: HistoryDbMode,
) -> io::Result<HistoryBrowserResult> {
    // Load history entries
    let entries = load_history(store)?;

    if entries.is_empty() {
        println!("# No history entries found.");
        return Ok(HistoryBrowserResult::Cancelled);
    }

    let mut browser = HistoryBrowser::new(entries, mode, store.clone());
    browser.run()
}

#[cfg(test)]
mod tests;
