//! Shared history lifecycle for code submitted through IPC.

use super::{HistoryExtraInfo, HistoryStore};
use reedline::{HistoryItem, HistoryItemId, HistorySessionId};

/// Build an IPC history item with metadata captured before evaluation starts.
///
/// Whitespace-only input is not a command and is not recorded.
pub(crate) fn ipc_history_item(
    code: &str,
    session_id: Option<HistorySessionId>,
) -> Option<HistoryItem> {
    if code.trim().is_empty() {
        return None;
    }

    let mut item = HistoryItem::from_command_line(code);
    item.start_timestamp = Some(chrono::Utc::now());
    item.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
    item.cwd = std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    item.session_id = session_id;
    Some(item)
}

/// A history row created when an IPC request begins and finalized once it ends.
///
/// Owning the store and row ID makes completing the lifecycle explicit. A
/// missing status remains unknown in history.
pub(crate) struct IpcHistoryEntry {
    store: HistoryStore,
    id: HistoryItemId,
}

impl IpcHistoryEntry {
    /// Persist a pending row immediately before starting the IPC evaluation.
    pub(crate) fn begin(
        store: &HistoryStore,
        code: &str,
        session_id: Option<HistorySessionId>,
    ) -> Option<Self> {
        let item = ipc_history_item(code, session_id)?;
        match store.save_known(item, HistoryExtraInfo::default()) {
            Ok(item) => item.id.map(|id| Self {
                store: store.clone(),
                id,
            }),
            Err(error) => {
                log::warn!("Failed to save IPC history: {error}");
                None
            }
        }
    }

    /// Return the history row ID for consumers that need to retain context.
    pub(crate) fn id(&self) -> HistoryItemId {
        self.id
    }

    /// Record a known result, leaving the status unset when it is unavailable.
    pub(crate) fn finish(self, exit_status: Option<i64>) {
        let Some(exit_status) = exit_status else {
            return;
        };
        if let Err(error) = self.store.set_exit_status(self.id, exit_status) {
            log::warn!(
                "Failed to update IPC history result for {}: {error}",
                self.id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::artifact::HistoryKind;

    fn store() -> (tempfile::TempDir, HistoryStore) {
        let dir = tempfile::tempdir().expect("create temporary history directory");
        let path = dir.path().join("r.db");
        let store =
            HistoryStore::open(path, HistoryKind::R, None, None).expect("open history store");
        (dir, store)
    }

    #[test]
    fn begin_persists_metadata_before_completion_and_finish_updates_once() {
        let (_dir, store) = store();
        let cwd = std::env::current_dir()
            .expect("get current directory")
            .to_string_lossy()
            .into_owned();
        let session_id = reedline::Reedline::create_history_session_id();

        let entry = IpcHistoryEntry::begin(&store, "setwd('elsewhere')", session_id)
            .expect("begin should persist the row");
        let id = entry.id();
        let pending = store
            .load_with_metadata(id)
            .expect("load pending history row");
        assert!(pending.start_timestamp.is_some());
        assert!(pending.hostname.is_some());
        assert_eq!(pending.cwd.as_deref(), Some(cwd.as_str()));
        assert_eq!(pending.session_id, session_id);
        assert_eq!(pending.exit_status, None);

        entry.finish(Some(1));
        let finished = store
            .load_with_metadata(id)
            .expect("load finished history row");
        assert_eq!(finished.exit_status, Some(1));
        assert_eq!(store.count_all().unwrap(), 1);
    }

    #[test]
    fn unavailable_result_remains_unknown() {
        let (_dir, store) = store();
        let entry = IpcHistoryEntry::begin(&store, "unknown_result()", None)
            .expect("begin should persist the row");
        let id = entry.id();

        entry.finish(None);

        assert_eq!(store.load(id).unwrap().exit_status, None);
    }

    #[test]
    fn whitespace_only_code_is_not_started_or_saved() {
        let (_dir, store) = store();

        assert!(IpcHistoryEntry::begin(&store, " \n\t", None).is_none());
        assert_eq!(store.count_all().unwrap(), 0);
    }
}
