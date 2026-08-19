//! Command history management.
//!
//! This module provides SQLite-backed command history storage
//! and fuzzy history search.

pub mod export;
pub mod import;
mod metadata;
mod reedline_adapter;
mod store;

pub use metadata::HistoryExtraInfo;
pub use reedline_adapter::ReedlineHistoryAdapter;
pub use store::{
    HistoryHandle, HistoryRuntime, HistorySaveOutcome, HistorySaveReceipt, HistoryStore,
    VolatileHistoryReason,
};
