//! History schema documentation for SQLite-backed command history.
//!
//! This module provides functions to display the history database schema
//! and example R code for accessing it, used by both CLI and REPL commands.

mod content;
mod lines;
mod print;
mod style;

use super::{PagerConfig, run};
use crate::config::{HistoryLocationSource, ResolvedHistoryLocation};
use content::schema_content;
use lines::generate_schema_lines;
use print::{print_schema_colored, print_schema_plain};
use std::io::{self, IsTerminal};
use std::path::Path;

/// Error returned when history directory cannot be determined.
#[derive(Debug)]
pub struct HistoryDirError {
    detail: Option<String>,
}

impl std::fmt::Display for HistoryDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "No persistent history directory is available")?;
        match &self.detail {
            Some(detail) => write!(f, ": {detail}"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for HistoryDirError {}

/// The effective directory, or the reason there is none.
fn history_directory(location: &ResolvedHistoryLocation) -> Result<&Path, HistoryDirError> {
    if location.source() == HistoryLocationSource::Volatile {
        return Err(HistoryDirError { detail: None });
    }
    location.directory().ok_or_else(|| HistoryDirError {
        detail: location.probe_failure().map(ToString::to_string),
    })
}

/// Print the history schema documentation to stdout.
///
/// This displays:
/// - Database file locations
/// - SQLite table schema
/// - Example R code for accessing the database
///
/// When stdout is not a terminal (e.g., piped), colors are disabled.
///
/// # Errors
///
/// Returns an error if the history directory cannot be determined.
pub fn print_schema(location: &ResolvedHistoryLocation) -> Result<(), HistoryDirError> {
    let history_path = history_directory(location)?;

    // Check if stdout is a terminal - only use colors if it is
    if io::stdout().is_terminal() {
        print_schema_colored(history_path);
    } else {
        print_schema_plain(history_path);
    }

    Ok(())
}

/// Display the history schema in an interactive pager (for REPL use).
///
/// This provides a scrollable view of the schema documentation.
/// Press `q`, `Esc`, or `Ctrl+C/D` to exit. Press `c` to copy R example.
///
/// # Errors
///
/// Returns an error if the history directory cannot be determined.
pub fn show_schema_pager(location: &ResolvedHistoryLocation) -> Result<(), HistoryDirError> {
    let history_path = history_directory(location)?;

    // Generate content lines
    let lines = generate_schema_lines(history_path);
    let mut content = schema_content(lines);

    // Configure pager
    let config = PagerConfig {
        title: "History Schema",
        footer_hint: "↑↓/jk scroll │ c copy R example │ q exit",
        manage_alternate_screen: true,
    };

    // Run the pager
    if let Err(e) = run(&mut content, &config) {
        eprintln!("Pager error: {}", e);
    }

    Ok(())
}

#[cfg(test)]
mod tests;
