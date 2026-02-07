//! Command history management.
//!
//! This module provides SQLite-backed command history storage
//! and fuzzy history search.

pub mod export;
pub mod import;
mod search;
mod storage;

pub use search::FuzzyHistory;
#[allow(unused_imports)]
pub use storage::HistoryExtraInfo;
