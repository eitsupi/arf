//! Command history management.
//!
//! This module provides SQLite-backed command history storage
//! and fuzzy history search.

mod search;
mod storage;

pub use search::FuzzyHistory;
