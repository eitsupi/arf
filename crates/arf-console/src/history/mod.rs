//! Command history management.
//!
//! This module provides SQLite-backed command history storage
//! and fuzzy history search.

pub(crate) mod artifact;
pub mod export;
pub mod import;
mod ipc;
mod metadata;
mod query;
mod reedline_adapter;
mod store;

pub(crate) use ipc::{IpcHistoryEntry, ipc_history_item};
pub use metadata::HistoryExtraInfo;
pub(crate) use query::{HistoryQuery, parse_since};
pub use reedline_adapter::ReedlineHistoryAdapter;
#[allow(unused_imports)]
pub use store::{
    HistoryFailureDetail, HistoryHandle, HistoryRuntime, HistorySaveOutcome, HistorySaveReceipt,
    HistoryStore, VolatileHistoryReason,
};
