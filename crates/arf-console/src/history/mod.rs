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
#[allow(unused_imports)]
pub use store::{
    HistoryFailureDetail, HistoryHandle, HistoryRuntime, HistorySaveOutcome, HistorySaveReceipt,
    HistoryStore, VolatileHistoryReason,
};
