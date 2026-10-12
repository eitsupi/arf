//! Owned SQLite history storage for arf.
//!
//! [`HistoryStore`] owns the concrete reedline database and is the only layer
//! that performs typed metadata writes.  Each method locks the database for a
//! single short call and releases the lock before returning.  Callers must not
//! hold a store lock across meta-command dispatch, R evaluation, a pager, a
//! confirmation prompt, or any other user interaction.

mod receipt;
mod repair;
mod runtime;

use super::artifact::{self, HistoryKind};
use super::metadata::HistoryExtraInfo;
use reedline::{
    History, HistoryItem, HistoryItemExtraInfo, HistoryItemId, HistorySessionId, Result,
    SearchQuery, SqliteBackedHistory,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use receipt::{HistorySaveOutcome, HistorySaveReceipt};
pub use runtime::{HistoryFailureDetail, HistoryHandle, HistoryRuntime, VolatileHistoryReason};

/// An arf-owned handle to one SQLite history database.
#[derive(Clone)]
pub struct HistoryStore {
    inner: Arc<Mutex<SqliteBackedHistory>>,
    path: Option<PathBuf>,
    session: Option<HistorySessionId>,
}

impl HistoryStore {
    /// Open or create a history database.
    pub(crate) fn open(
        path: PathBuf,
        history_kind: HistoryKind,
        session_id: Option<HistorySessionId>,
        session_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Self> {
        artifact::prepare_history_path(&path, history_kind).map_err(|error| {
            reedline::ReedlineError(reedline::ReedlineErrorVariants::HistoryDatabaseError(
                format!("invalid history database artifact: {error:#}"),
            ))
        })?;
        let history = SqliteBackedHistory::with_file(path.clone(), session_id, session_timestamp)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(history)),
            path: Some(path),
            session: session_id,
        })
    }

    /// Create an arf-owned in-memory SQLite history store.
    pub fn in_memory(
        session_id: Option<HistorySessionId>,
        _session_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(SqliteBackedHistory::in_memory()?)),
            path: None,
            session: session_id,
        })
    }

    /// Compare ownership identity without opening another backend.
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// Save an item with SQL `NULL` in `more_info`.
    pub fn save_unknown(&self, item: HistoryItem) -> Result<HistoryItem> {
        let mut item = item;
        if item.session_id.is_none() {
            item.session_id = self.session;
        }
        let typed = convert_history_item(item, None::<HistoryExtraInfo>);
        let saved = self
            .inner
            .lock()
            .map_err(|_| lock_error())?
            .save_with_extra(typed)?;
        Ok(convert_history_item(saved, None))
    }

    /// Save an item with metadata already determined by arf.
    pub fn save_known(&self, item: HistoryItem, metadata: HistoryExtraInfo) -> Result<HistoryItem> {
        let mut item = item;
        if item.session_id.is_none() {
            item.session_id = self.session;
        }
        let typed = convert_history_item(item, Some(metadata));
        let saved = self
            .inner
            .lock()
            .map_err(|_| lock_error())?
            .save_with_extra(typed)?;
        Ok(convert_history_item(saved, None))
    }

    /// Save a fully typed item imported from an external history source.
    pub(crate) fn save_imported(
        &self,
        item: HistoryItem<HistoryExtraInfo>,
    ) -> Result<HistoryItem<HistoryExtraInfo>> {
        let mut item = item;
        if item.session_id.is_none() {
            item.session_id = self.session;
        }
        self.inner
            .lock()
            .map_err(|_| lock_error())?
            .save_with_extra(item)
    }

    /// Set the final meta-command disposition while preserving all other fields.
    pub fn finalize_meta_command(&self, id: HistoryItemId, is_meta_command: bool) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| lock_error())?
            .update_with_extra::<HistoryExtraInfo>(id, &|mut item| {
                let mut metadata: HistoryExtraInfo = item.more_info.unwrap_or_default();
                metadata.set_meta_command(is_meta_command);
                item.more_info = Some(metadata);
                item
            })
    }

    /// Replace the metadata object of an existing row.
    #[allow(dead_code)]
    pub fn set_metadata(&self, id: HistoryItemId, metadata: HistoryExtraInfo) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| lock_error())?
            .update_with_extra::<HistoryExtraInfo>(id, &|mut item| {
                item.more_info = Some(metadata.clone());
                item
            })
    }

    /// Set the exit status of an existing row.
    pub fn set_exit_status(&self, id: HistoryItemId, exit_status: i64) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| lock_error())?
            .update_with_extra::<HistoryExtraInfo>(id, &|mut item| {
                item.exit_status = Some(exit_status);
                item
            })
    }

    /// Count all rows without exposing the concrete reedline backend.
    pub fn count_all(&self) -> Result<i64> {
        self.count(SearchQuery::everything(
            reedline::SearchDirection::Backward,
            None,
        ))
    }

    pub fn load(&self, id: HistoryItemId) -> Result<HistoryItem> {
        self.inner.lock().map_err(|_| lock_error())?.load(id)
    }

    #[allow(dead_code)]
    pub fn load_with_metadata(&self, id: HistoryItemId) -> Result<HistoryItem<HistoryExtraInfo>> {
        self.inner
            .lock()
            .map_err(|_| lock_error())?
            .load_with_extra(id)
    }

    pub fn count(&self, query: SearchQuery) -> Result<i64> {
        self.inner.lock().map_err(|_| lock_error())?.count(query)
    }

    pub fn search(&self, query: SearchQuery) -> Result<Vec<HistoryItem>> {
        self.inner.lock().map_err(|_| lock_error())?.search(query)
    }

    /// Search with an exact session scope and a result limit.
    ///
    /// Reedline's session filter intentionally includes rows from before the
    /// current session, which is useful for interactive recall but not for the
    /// IPC contract. This owned API applies exact filtering after each bounded
    /// backend page and keeps scanning by ID until the requested number of rows
    /// is collected. Callers never need to materialize the whole database.
    pub(crate) fn search_strict_session<F>(
        &self,
        mut make_query: F,
        session_id: Option<HistorySessionId>,
        all_sessions: bool,
        limit: i64,
        start_time: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<HistoryItem>>
    where
        F: FnMut(Option<HistoryItemId>) -> SearchQuery,
    {
        const PAGE_SIZE: i64 = 128;
        if limit <= 0 {
            return Ok(Vec::new());
        }

        if all_sessions && start_time.is_none() {
            let mut query = make_query(None);
            query.limit = Some(limit);
            return self.search(query);
        }

        let mut matched = Vec::new();
        let mut start_id = None;
        loop {
            let mut query = make_query(start_id);
            // Never let reedline apply its two-stage session semantics here.
            query.filter.session = None;
            query.limit = Some(PAGE_SIZE);
            let page = self.search(query)?;
            let page_len = page.len();
            let next_start_id = page.last().and_then(|item| item.id);
            matched.extend(page.into_iter().filter(|item| {
                (all_sessions || item.session_id == session_id)
                    && start_time.is_none_or(|start| {
                        item.start_timestamp
                            .is_some_and(|timestamp| timestamp >= start)
                    })
            }));
            if matched.len() >= limit as usize || page_len < PAGE_SIZE as usize {
                matched.truncate(limit as usize);
                return Ok(matched);
            }
            let Some(next_start_id) = next_start_id else {
                matched.truncate(limit as usize);
                return Ok(matched);
            };
            start_id = Some(next_start_id);
        }
    }

    pub fn update(
        &self,
        id: HistoryItemId,
        updater: &dyn Fn(HistoryItem) -> HistoryItem,
    ) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| lock_error())?
            .update(id, updater)
    }

    pub fn clear(&self) -> Result<()> {
        self.inner.lock().map_err(|_| lock_error())?.clear()
    }

    pub fn delete(&self, id: HistoryItemId) -> Result<()> {
        self.inner.lock().map_err(|_| lock_error())?.delete(id)
    }

    /// Delete several rows while holding the owning backend lock once.
    pub(crate) fn delete_many(&self, ids: &[HistoryItemId]) -> Result<usize> {
        let mut history = self.inner.lock().map_err(|_| lock_error())?;
        let mut deleted = 0;
        for id in ids {
            history.delete(*id)?;
            deleted += 1;
        }
        Ok(deleted)
    }

    pub fn sync(&self) -> std::io::Result<()> {
        self.inner
            .lock()
            .map_err(|_| std::io::Error::other("history store lock poisoned"))?
            .sync()
    }

    pub fn session(&self) -> Option<HistorySessionId> {
        self.session
    }

    #[cfg(test)]
    pub(crate) fn drop_table_for_test(&self, path: &std::path::Path) {
        rusqlite::Connection::open(path)
            .unwrap()
            .execute_batch("DROP TABLE history")
            .unwrap();
    }
}

/// Convert a history item between reedline's type-erased and typed boundaries.
pub(crate) fn convert_history_item<A: HistoryItemExtraInfo, B: HistoryItemExtraInfo>(
    item: HistoryItem<A>,
    more_info: Option<B>,
) -> HistoryItem<B> {
    HistoryItem {
        id: item.id,
        start_timestamp: item.start_timestamp,
        command_line: item.command_line,
        session_id: item.session_id,
        hostname: item.hostname,
        cwd: item.cwd,
        duration: item.duration,
        exit_status: item.exit_status,
        more_info,
    }
}

fn lock_error() -> reedline::ReedlineError {
    reedline::ReedlineError(reedline::ReedlineErrorVariants::HistoryDatabaseError(
        "history store lock poisoned".to_string(),
    ))
}

#[cfg(test)]
mod tests;
