//! The IPC `history` method: maps request parameters onto the history store.

use super::protocol::{HistoryEntry, HistoryParams, HistoryResult};
use crate::history::{HistoryQuery, HistoryStore, parse_since};
use std::sync::OnceLock;

/// The single history owner shared by REPL/headless history queries and saves.
static HISTORY_STORE: OnceLock<HistoryStore> = OnceLock::new();

/// Register the already-open history owner for IPC queries.
pub fn set_history_store(history: HistoryStore) {
    if HISTORY_STORE.set(history).is_err() {
        log::warn!("History store already initialized; ignoring duplicate set_history_store call");
    }
}

/// Error type for history query failures, distinguishing parameter
/// validation errors from internal/database errors.
pub(crate) enum HistoryQueryError {
    /// Client provided invalid parameters (maps to INVALID_PARAMS).
    InvalidParams(String),
    /// Internal failure such as database open/query error (maps to INTERNAL_ERROR).
    Internal(String),
}

/// Query the history store and return matching entries.
///
/// Queries use the already-open owner, avoiding competing WAL/DDL connections
/// and making volatile history available to IPC for the current session.
pub(crate) fn query_history(params: &HistoryParams) -> Result<HistoryResult, HistoryQueryError> {
    if params.limit < 1 {
        return Err(HistoryQueryError::InvalidParams(format!(
            "limit must be positive, got {}",
            params.limit
        )));
    }

    let store = HISTORY_STORE.get().ok_or_else(|| {
        HistoryQueryError::Internal(
            "History is not available (no history store configured)".to_string(),
        )
    })?;
    let session_id = store.session();

    // When not requesting all sessions, require a valid session_id.
    if !params.all_sessions && session_id.is_none() {
        return Err(HistoryQueryError::Internal(
            "Session ID is not available; use all_sessions=true to query without session scope"
                .to_string(),
        ));
    }

    // Parse --since before opening the DB so validation errors are cheap.
    let since = params
        .since
        .as_deref()
        .map(|since| {
            parse_since(since).map_err(|_| {
                HistoryQueryError::InvalidParams(format!(
                    "Invalid 'since' format: {since}. \
                     Use RFC 3339 (e.g. '2026-03-29T00:00:00Z') or date (e.g. '2026-03-29')"
                ))
            })
        })
        .transpose()?;

    let rows = store
        .query(&HistoryQuery {
            limit: params.limit,
            all_sessions: params.all_sessions,
            cwd: params.cwd.clone(),
            grep: params.grep.clone(),
            since,
        })
        .map_err(|e| {
            log::error!("Strict history query failed: {e}");
            HistoryQueryError::Internal(format!("History query failed: {e}"))
        })?;
    let entries = rows
        .into_iter()
        .map(|row| HistoryEntry {
            command: row.command_line,
            timestamp: row.start_timestamp.map(|time| time.to_rfc3339()),
            cwd: row.cwd,
            exit_status: row.exit_status,
            session_id: row.session_id.map(i64::from),
        })
        .collect();

    Ok(HistoryResult {
        entries,
        session_id: session_id.map(i64::from),
    })
}
