//! Import-only repair of legacy rows through a raw SQLite transaction.

use super::{HistoryStore, lock_error};
use crate::history::HistoryExtraInfo;
use reedline::{HistoryItem, HistoryItemId, Result};

fn sqlite_error(error: rusqlite::Error) -> reedline::ReedlineError {
    reedline::ReedlineError(reedline::ReedlineErrorVariants::HistoryDatabaseError(
        format!("{error:?}"),
    ))
}

impl HistoryStore {
    /// Fill missing imported fields only if each row field is still SQL NULL.
    pub(crate) fn set_missing_fields_if_empty(
        &self,
        id: HistoryItemId,
        source: HistoryItem<HistoryExtraInfo>,
    ) -> Result<bool> {
        // This is the one import-only escape hatch from reedline's typed API.
        // Holding the store mutex prevents the raw transaction from racing an
        // adapter write; the helper itself never escapes this ownership boundary.
        let _store_lock = self.inner.lock().map_err(|_| lock_error())?;
        self.set_missing_fields_with_raw_transaction(id, source)
    }

    /// Backfill legacy rows whose `more_info` may contain malformed JSON.
    ///
    /// Reedline's typed loader intentionally rejects malformed metadata, so
    /// import repair must use SQL COALESCE while the owning store is locked.
    fn set_missing_fields_with_raw_transaction(
        &self,
        id: HistoryItemId,
        source: HistoryItem<HistoryExtraInfo>,
    ) -> Result<bool> {
        let serialized_metadata = source
            .more_info
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                reedline::ReedlineError(reedline::ReedlineErrorVariants::HistoryDatabaseError(
                    format!("could not serialize more_info: {error}"),
                ))
            })?;
        let Some(path) = self.path.as_ref() else {
            return Ok(false);
        };
        let mut connection = rusqlite::Connection::open(path).map_err(sqlite_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sqlite_error)?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let changed = transaction
            .execute(
                "UPDATE history SET
                    session_id = COALESCE(session_id, :session_id),
                    hostname = COALESCE(hostname, :hostname),
                    cwd = COALESCE(cwd, :cwd),
                    duration_ms = COALESCE(duration_ms, :duration_ms),
                    exit_status = COALESCE(exit_status, :exit_status),
                    more_info = COALESCE(more_info, :more_info)
                 WHERE id = :id
                   AND command_line = :command_line
                   AND (
                       :start_timestamp IS NULL
                    OR start_timestamp = :start_timestamp
                   )
                   AND (
                       (:session_id IS NOT NULL AND session_id IS NULL)
                    OR (:hostname IS NOT NULL AND hostname IS NULL)
                    OR (:cwd IS NOT NULL AND cwd IS NULL)
                    OR (:duration_ms IS NOT NULL AND duration_ms IS NULL)
                    OR (:exit_status IS NOT NULL AND exit_status IS NULL)
                    OR (:more_info IS NOT NULL AND more_info IS NULL)
                   )",
                rusqlite::named_params! {
                    ":id": id.0,
                    ":command_line": source.command_line,
                    ":start_timestamp": source.start_timestamp.map(|value| value.timestamp_millis()),
                    ":session_id": source.session_id.map(i64::from),
                    ":hostname": source.hostname,
                    ":cwd": source.cwd,
                    ":duration_ms": source.duration.map(|value| value.as_millis() as i64),
                    ":exit_status": source.exit_status,
                    ":more_info": serialized_metadata,
                },
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok(changed != 0)
    }
}
