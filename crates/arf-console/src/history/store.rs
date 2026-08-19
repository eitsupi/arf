//! Owned SQLite history storage for arf.
//!
//! [`HistoryStore`] owns the concrete reedline database and is the only layer
//! that performs typed metadata writes.  Each method locks the database for a
//! single short call and releases the lock before returning.  Callers must not
//! hold a store lock across meta-command dispatch, R evaluation, a pager, a
//! confirmation prompt, or any other user interaction.

use super::metadata::HistoryExtraInfo;
use reedline::{
    History, HistoryItem, HistoryItemExtraInfo, HistoryItemId, HistorySessionId, Reedline, Result,
    SearchQuery, SqliteBackedHistory,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// An arf-owned handle to one SQLite history database.
#[derive(Clone)]
pub struct HistoryStore {
    inner: Arc<Mutex<SqliteBackedHistory>>,
    path: Option<PathBuf>,
    session: Option<HistorySessionId>,
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

    pub fn take(&self) -> Option<HistorySaveOutcome> {
        self.inner
            .lock()
            .ok()
            .and_then(|mut outcome| outcome.take())
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

/// Runtime history lifecycle. Keeping this state explicit prevents a failed
/// persistent open from being confused with an intentional volatile session.
#[derive(Clone)]
pub enum HistoryRuntime {
    Persistent(HistoryHandle),
    Volatile {
        handle: HistoryHandle,
        reason: VolatileHistoryReason,
    },
    Unavailable {
        failure: HistoryFailureDetail,
        previous_failure: Option<HistoryFailureDetail>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VolatileHistoryReason {
    Configured,
    Fallback {
        persistent_failure: HistoryFailureDetail,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryFailureDetail {
    stage: HistoryFailureStage,
    path: Option<PathBuf>,
    message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HistoryFailureStage {
    PersistentPathResolution,
    PersistentOpen,
    MemoryInitialization,
}

impl HistoryRuntime {
    /// Construct the complete history lifecycle decision for one runtime.
    ///
    /// Persistent open failures (including an unavailable default path) are
    /// deliberately represented as volatile fallbacks.  Only failure to
    /// create that fallback reaches `Unavailable`.
    pub fn initialize(
        mode: &crate::config::HistoryMode,
        requested_path: Option<PathBuf>,
        session_id: Option<HistorySessionId>,
        session_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        Self::initialize_with_factories(
            mode,
            requested_path,
            session_id,
            session_timestamp,
            HistoryStore::open,
            HistoryStore::in_memory,
        )
    }

    /// Apply the lifecycle decision with injectable backend factories.
    ///
    /// The factories are kept private so production callers cannot bypass the
    /// single runtime construction path. Unit tests use this helper to cover
    /// initialization failures that the operating system cannot reliably
    /// reproduce on every platform.
    fn initialize_with_factories<Open, Memory>(
        mode: &crate::config::HistoryMode,
        requested_path: Option<PathBuf>,
        session_id: Option<HistorySessionId>,
        session_timestamp: Option<chrono::DateTime<chrono::Utc>>,
        mut open: Open,
        mut in_memory: Memory,
    ) -> Self
    where
        Open: FnMut(
            PathBuf,
            Option<HistorySessionId>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<HistoryStore>,
        Memory: FnMut(
            Option<HistorySessionId>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<HistoryStore>,
    {
        let make_handle = |store: HistoryStore| HistoryHandle {
            store,
            receipt: HistorySaveReceipt::new(),
        };

        if matches!(mode, crate::config::HistoryMode::Volatile) {
            return match in_memory(session_id, session_timestamp) {
                Ok(store) => Self::Volatile {
                    handle: make_handle(store),
                    reason: VolatileHistoryReason::Configured,
                },
                Err(error) => Self::Unavailable {
                    failure: HistoryFailureDetail::memory(error),
                    previous_failure: None,
                },
            };
        }

        let persistent_failure = match requested_path.clone() {
            Some(path) => match open(path.clone(), session_id, session_timestamp) {
                Ok(store) => return Self::Persistent(make_handle(store)),
                Err(error) => HistoryFailureDetail::persistent_open(path, error),
            },
            None => HistoryFailureDetail::persistent_path(),
        };

        match in_memory(session_id, session_timestamp) {
            Ok(store) => Self::Volatile {
                handle: make_handle(store),
                reason: VolatileHistoryReason::Fallback { persistent_failure },
            },
            Err(error) => Self::Unavailable {
                failure: HistoryFailureDetail::memory(error),
                previous_failure: Some(persistent_failure),
            },
        }
    }

    /// Install this runtime's owned adapter on an editor, if available.
    pub fn attach_to_editor(&self, line_editor: Reedline) -> Reedline {
        let Some(handle) = self.handle() else {
            return line_editor;
        };
        let adapter =
            super::ReedlineHistoryAdapter::new(handle.store.clone(), handle.receipt.clone());
        line_editor
            .with_history_session_id(handle.store.session())
            .with_history(Box::new(adapter))
    }

    pub fn handle(&self) -> Option<&HistoryHandle> {
        match self {
            Self::Persistent(handle) | Self::Volatile { handle, .. } => Some(handle),
            Self::Unavailable { .. } => None,
        }
    }

    pub fn store(&self) -> Option<HistoryStore> {
        self.handle().map(|handle| handle.store.clone())
    }

    pub fn receipt_outcome(&self) -> Option<HistorySaveOutcome> {
        self.handle().and_then(|handle| handle.receipt.take())
    }

    pub fn is_available(&self) -> bool {
        self.handle().is_some()
    }

    /// Stable state label for diagnostics and machine-readable startup info.
    pub fn state_name(&self) -> &'static str {
        match self {
            Self::Persistent(_) => "persistent",
            Self::Volatile { .. } => "volatile",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Stable machine-readable reason for a non-persistent runtime.
    pub fn reason_name(&self) -> Option<&'static str> {
        match self {
            Self::Persistent(_) => None,
            Self::Volatile { reason, .. } => Some(match reason {
                VolatileHistoryReason::Configured => "configured",
                VolatileHistoryReason::Fallback { .. } => "fallback",
            }),
            Self::Unavailable { .. } => Some("initialization_failed"),
        }
    }

    /// Human-readable startup warning for degraded runtimes.
    ///
    /// Persistent and intentionally configured volatile runtimes are healthy
    /// states and therefore do not produce a warning.
    pub fn startup_warning(&self) -> Option<String> {
        let detail = self.diagnostic_detail()?;
        Some(match self.requested_path() {
            Some(path) => format!("{detail} (requested path: {})", path.display()),
            None => detail,
        })
    }

    /// Human-readable detail shared by startup warnings, `:info`, and JSON.
    pub fn diagnostic_detail(&self) -> Option<String> {
        match self {
            Self::Persistent(_) => None,
            Self::Volatile { reason, .. } => match reason {
                VolatileHistoryReason::Configured => None,
                VolatileHistoryReason::Fallback { persistent_failure } => Some(format!(
                    "{}; using volatile fallback",
                    persistent_failure.describe(),
                )),
            },
            Self::Unavailable {
                failure,
                previous_failure,
            } => {
                let mut details = previous_failure
                    .as_ref()
                    .map(|failure| failure.describe())
                    .unwrap_or_default();
                if !details.is_empty() {
                    details.push_str("; ");
                }
                details.push_str(&failure.describe());
                Some(details)
            }
        }
    }

    pub fn requested_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Persistent(handle) => handle.store.path(),
            Self::Volatile { reason, .. } => match reason {
                VolatileHistoryReason::Configured => None,
                VolatileHistoryReason::Fallback { persistent_failure } => persistent_failure.path(),
            },
            Self::Unavailable {
                previous_failure, ..
            } => previous_failure
                .as_ref()
                .and_then(HistoryFailureDetail::path),
        }
    }
}

impl HistoryFailureDetail {
    #[cfg(test)]
    pub(crate) fn test_memory() -> Self {
        Self {
            stage: HistoryFailureStage::MemoryInitialization,
            path: None,
            message: "test memory initialization failure".to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_persistent_open(path: PathBuf) -> Self {
        Self {
            stage: HistoryFailureStage::PersistentOpen,
            path: Some(path),
            message: "test persistent open failure".to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_path_resolution() -> Self {
        Self::persistent_path()
    }

    fn persistent_path() -> Self {
        Self {
            stage: HistoryFailureStage::PersistentPathResolution,
            path: None,
            message: "no persistent history path is available".to_string(),
        }
    }

    fn persistent_open(path: PathBuf, error: reedline::ReedlineError) -> Self {
        Self {
            stage: HistoryFailureStage::PersistentOpen,
            path: Some(path),
            message: error.to_string(),
        }
    }

    fn memory(error: reedline::ReedlineError) -> Self {
        Self {
            stage: HistoryFailureStage::MemoryInitialization,
            path: None,
            message: error.to_string(),
        }
    }

    fn describe(&self) -> String {
        match self.stage {
            HistoryFailureStage::PersistentPathResolution => {
                format!(
                    "persistent history path resolution failed: {}",
                    self.message
                )
            }
            HistoryFailureStage::PersistentOpen => {
                format!("persistent history open failed: {}", self.message)
            }
            HistoryFailureStage::MemoryInitialization => {
                format!("volatile history initialization failed: {}", self.message)
            }
        }
    }

    fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
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
                path.clone(),
                session_id,
                session_timestamp,
            )?)),
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

fn sqlite_error(error: rusqlite::Error) -> reedline::ReedlineError {
    reedline::ReedlineError(reedline::ReedlineErrorVariants::HistoryDatabaseError(
        format!("{error:?}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn injected_failure(message: &str) -> Result<HistoryStore> {
        Err(reedline::ReedlineError(
            reedline::ReedlineErrorVariants::HistoryDatabaseError(message.to_string()),
        ))
    }

    fn open_store() -> (tempfile::TempDir, HistoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path().join("history.db"), None, None).unwrap();
        (dir, store)
    }

    #[test]
    fn configured_volatile_memory_failure_is_unavailable_without_previous_failure() {
        let runtime = HistoryRuntime::initialize_with_factories(
            &crate::config::HistoryMode::Volatile,
            None,
            None,
            None,
            |_path, _session, _timestamp| injected_failure("persistent must not be opened"),
            |_session, _timestamp| injected_failure("configured memory failure"),
        );

        assert!(runtime.requested_path().is_none());
        let HistoryRuntime::Unavailable {
            failure,
            previous_failure,
        } = runtime
        else {
            panic!("configured volatile initialization should be unavailable");
        };
        assert_eq!(failure.stage, HistoryFailureStage::MemoryInitialization);
        assert!(failure.message.contains("configured memory failure"));
        assert!(previous_failure.is_none());
    }

    #[test]
    fn persistent_failure_and_memory_failure_are_both_retained() {
        let requested_path = PathBuf::from("/requested/history.db");
        let runtime = HistoryRuntime::initialize_with_factories(
            &crate::config::HistoryMode::Persistent { dir: None },
            Some(requested_path.clone()),
            None,
            None,
            |_path, _session, _timestamp| injected_failure("persistent open failure"),
            |_session, _timestamp| injected_failure("fallback memory failure"),
        );

        let detail = runtime.diagnostic_detail().expect("unavailable detail");
        assert_eq!(runtime.requested_path(), Some(requested_path.as_path()));
        let HistoryRuntime::Unavailable {
            failure,
            previous_failure,
        } = runtime
        else {
            panic!("persistent and fallback initialization should be unavailable");
        };
        assert_eq!(failure.stage, HistoryFailureStage::MemoryInitialization);
        assert!(failure.message.contains("fallback memory failure"));
        let previous_failure = previous_failure.expect("persistent failure should be retained");
        assert_eq!(previous_failure.stage, HistoryFailureStage::PersistentOpen);
        assert!(previous_failure.message.contains("persistent open failure"));
        assert!(detail.contains("fallback memory failure"));
        let requested_path_text = requested_path.to_string_lossy();
        assert_eq!(detail.matches(requested_path_text.as_ref()).count(), 0);
        let warning = HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: Some(HistoryFailureDetail::test_persistent_open(
                requested_path.clone(),
            )),
        }
        .startup_warning()
        .expect("unavailable warning");
        assert_eq!(warning.matches(requested_path_text.as_ref()).count(), 1);
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
    fn strict_session_search_pages_before_applying_limit() {
        let current = reedline::Reedline::create_history_session_id().unwrap();
        let other = reedline::Reedline::create_history_session_id().unwrap();
        let store = HistoryStore::in_memory(Some(current), None).unwrap();

        for command in ["current old", "current new"] {
            let mut item = HistoryItem::from_command_line(command);
            item.session_id = Some(current);
            store.save_unknown(item).unwrap();
        }
        for index in 0..128 {
            let mut item = HistoryItem::from_command_line(format!("other {index}"));
            item.session_id = Some(other);
            store.save_unknown(item).unwrap();
        }

        let rows = store
            .search_strict_session(
                |start_id| SearchQuery {
                    direction: reedline::SearchDirection::Backward,
                    start_time: None,
                    end_time: None,
                    start_id,
                    end_id: None,
                    limit: None,
                    filter: reedline::SearchFilter::anything(None),
                },
                Some(current),
                false,
                2,
                None,
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|item| item.session_id == Some(current)));
    }

    #[test]
    fn strict_session_search_handles_future_time_without_rows() {
        let current = reedline::Reedline::create_history_session_id().unwrap();
        let store = HistoryStore::in_memory(Some(current), None).unwrap();
        let rows = store
            .search_strict_session(
                |start_id| SearchQuery {
                    direction: reedline::SearchDirection::Backward,
                    start_time: None,
                    end_time: None,
                    start_id,
                    end_id: None,
                    limit: None,
                    filter: reedline::SearchFilter::anything(None),
                },
                Some(current),
                false,
                50,
                Some(
                    chrono::DateTime::parse_from_rfc3339("2999-01-01T00:00:00Z")
                        .unwrap()
                        .with_timezone(&chrono::Utc),
                ),
            )
            .unwrap();
        assert!(rows.is_empty());
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
    fn taking_a_recorded_outcome_consumes_it() {
        let receipt = HistorySaveReceipt::new();
        let outcome = HistorySaveOutcome::Saved(reedline::HistoryItemId::new(42));
        receipt.record(outcome);

        assert_eq!(receipt.take(), Some(outcome));
        assert_eq!(receipt.take(), None);
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
