//! Owned SQLite history storage for arf.
//!
//! [`HistoryStore`] owns the concrete reedline database and is the only layer
//! that performs typed metadata writes.  Each method locks the database for a
//! single short call and releases the lock before returning.  Callers must not
//! hold a store lock across meta-command dispatch, R evaluation, a pager, a
//! confirmation prompt, or any other user interaction.

use super::metadata::HistoryExtraInfo;
use reedline::{
    History, HistoryItem, HistoryItemExtraInfo, HistoryItemId, HistorySessionId, Result,
    SearchQuery, SqliteBackedHistory,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// An arf-owned handle to one SQLite history database.
#[derive(Clone)]
pub struct HistoryStore {
    inner: Arc<Mutex<SqliteBackedHistory>>,
}

/// The result of the most recent adapter save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistorySaveOutcome {
    Saved(HistoryItemId),
    Failed,
}

/// Shared receipt through which arf can observe an adapter save result.
#[derive(Clone)]
pub struct HistorySaveReceipt {
    inner: Arc<Mutex<Option<HistorySaveOutcome>>>,
}

impl HistorySaveReceipt {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    pub fn latest(&self) -> Option<HistorySaveOutcome> {
        self.inner.lock().ok().and_then(|outcome| *outcome)
    }

    pub(crate) fn record(&self, outcome: HistorySaveOutcome) {
        if let Ok(mut latest) = self.inner.lock() {
            *latest = Some(outcome);
        }
    }
}

/// The store and save receipt associated with one reedline editor.
#[derive(Clone)]
pub struct HistoryHandle {
    pub store: HistoryStore,
    pub receipt: HistorySaveReceipt,
}

impl HistoryStore {
    /// Open or create a history database.
    pub fn open(
        path: PathBuf,
        session_id: Option<HistorySessionId>,
        session_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(SqliteBackedHistory::with_file(
                path,
                session_id,
                session_timestamp,
            )?)),
        })
    }

    /// Save an item with SQL `NULL` in `more_info`.
    pub fn save_unknown(&self, item: HistoryItem) -> Result<HistoryItem> {
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
        let typed = convert_history_item(item, Some(metadata));
        let saved = self
            .inner
            .lock()
            .map_err(|_| lock_error())?
            .save_with_extra(typed)?;
        Ok(convert_history_item(saved, None))
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
            .update(id, &|mut item| {
                item.exit_status = Some(exit_status);
                item
            })
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

    pub fn sync(&self) -> std::io::Result<()> {
        self.inner
            .lock()
            .map_err(|_| std::io::Error::other("history store lock poisoned"))?
            .sync()
    }

    pub fn session(&self) -> Option<HistorySessionId> {
        self.inner.lock().ok().and_then(|history| history.session())
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
mod tests {
    use super::*;
    use serde_json::Value;

    fn open_store() -> (tempfile::TempDir, HistoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path().join("history.db"), None, None).unwrap();
        (dir, store)
    }

    #[test]
    fn unknown_and_known_saves_have_expected_metadata() {
        let (_dir, store) = open_store();
        let unknown = store
            .save_unknown(HistoryItem::from_command_line(":not dispatched"))
            .unwrap();
        let known = store
            .save_known(
                HistoryItem::from_command_line("x <- 1:10"),
                HistoryExtraInfo::default(),
            )
            .unwrap();

        let unknown_typed = store
            .inner
            .lock()
            .unwrap()
            .load_with_extra::<HistoryExtraInfo>(unknown.id.unwrap())
            .unwrap();
        let known_typed = store
            .inner
            .lock()
            .unwrap()
            .load_with_extra::<HistoryExtraInfo>(known.id.unwrap())
            .unwrap();
        assert_eq!(unknown_typed.more_info, None);
        assert_eq!(known_typed.more_info, Some(HistoryExtraInfo::default()));
    }

    #[test]
    fn typed_finalization_preserves_future_fields() {
        let (_dir, store) = open_store();
        let item = store
            .save_known(
                HistoryItem::from_command_line("future"),
                serde_json::from_str(r#"{"future":true}"#).unwrap(),
            )
            .unwrap();
        store.finalize_meta_command(item.id.unwrap(), true).unwrap();
        let stored = store
            .inner
            .lock()
            .unwrap()
            .load_with_extra::<HistoryExtraInfo>(item.id.unwrap())
            .unwrap();
        let metadata = stored.more_info.unwrap();
        assert_eq!(metadata.meta_command(), Some(true));
        let fields: serde_json::Map<String, Value> =
            serde_json::from_str(&serde_json::to_string(&metadata).unwrap()).unwrap();
        assert_eq!(fields.get("future"), Some(&Value::Bool(true)));
    }

    #[test]
    fn failed_finalization_does_not_write_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let store = HistoryStore::open(path.clone(), None, None).unwrap();
        let item = store
            .save_unknown(HistoryItem::from_command_line("will be cleared"))
            .unwrap();
        let id = item.id.unwrap();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(
                r#"CREATE TRIGGER fail_finalize
                   BEFORE UPDATE OF more_info ON history
                   BEGIN SELECT RAISE(ABORT, 'test failure'); END;"#,
            )
            .unwrap();
        assert!(store.finalize_meta_command(id, false).is_err());
        let connection = rusqlite::Connection::open(path).unwrap();
        let raw: Option<String> = connection
            .query_row(
                "SELECT more_info FROM history WHERE id = ?",
                [id.0],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw, None);
    }

    #[test]
    fn explicit_status_update_targets_the_outer_row_after_a_menu_row() {
        let (_dir, store) = open_store();
        let outer = store
            .save_unknown(HistoryItem::from_command_line("readline()"))
            .unwrap();
        let menu = store
            .save_unknown(HistoryItem::from_command_line(r#":menu choice"#))
            .unwrap();
        store.set_exit_status(outer.id.unwrap(), 1).unwrap();

        assert_eq!(store.load(outer.id.unwrap()).unwrap().exit_status, Some(1));
        assert_eq!(store.load(menu.id.unwrap()).unwrap().exit_status, None);
    }
}
