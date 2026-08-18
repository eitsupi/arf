//! REPL history setup and persistence helpers.

use crate::history::{
    HistoryExtraInfo, HistoryHandle, HistorySaveOutcome, HistoryStore, ReedlineHistoryAdapter,
};
use reedline::{History, HistoryItem, HistoryItemId, HistorySessionId, Reedline};

/// Set up history for a line editor with a specific database path.
///
/// The optional handle is absent when no persistent path was configured or
/// when the database could not be opened.  Reedline still retains its default
/// history implementation in either case.
pub(super) fn setup_history(
    line_editor: Reedline,
    history_path: Option<std::path::PathBuf>,
    session_id: Option<HistorySessionId>,
) -> (Reedline, Option<HistoryHandle>) {
    let Some(path) = history_path else {
        return (line_editor, None);
    };

    match HistoryStore::open(path.clone(), session_id, Some(chrono::Utc::now())) {
        Ok(store) => {
            let receipt = crate::history::HistorySaveReceipt::new();
            let adapter = ReedlineHistoryAdapter::new(store.clone(), receipt.clone());
            let handle = HistoryHandle { store, receipt };
            let editor = line_editor
                .with_history_session_id(session_id)
                .with_history(Box::new(adapter));
            (editor, Some(handle))
        }
        Err(error) => {
            log::warn!(
                "Failed to open history database {}: {}",
                path.display(),
                error
            );
            (line_editor, None)
        }
    }
}

/// Save code injected into an interactive session with known ordinary-command
/// metadata.  History failures are non-fatal to the IPC operation.
///
/// Without a persistent store, reedline keeps its own default backend — an
/// in-memory ring buffer — and ordinary typed input still lands there, so IPC
/// code must too or it silently drops out of in-session recall. That backend is
/// the only writer in that case, so the two branches must stay exclusive:
/// saving through the editor while the adapter is installed would write to
/// SQLite a second time.
///
/// No ID is returned for the in-memory branch. `FileBackedHistory` hands back a
/// deque index that shifts as the buffer evicts old entries, refuses `update`
/// outright, and returns nothing at all when the entry repeats the previous
/// one — so the value cannot address a row later.
pub(super) fn save_ipc_history(
    history: &mut dyn History,
    store: Option<HistoryStore>,
    code: &str,
    session_id: Option<HistorySessionId>,
) -> Option<HistoryItemId> {
    if code.trim().is_empty() {
        return None;
    }

    let Some(store) = store else {
        let mut item = HistoryItem::from_command_line(code);
        item.start_timestamp = Some(chrono::Utc::now());
        item.session_id = session_id;
        if let Err(error) = history.save(item) {
            log::warn!("Failed to save IPC history to the in-memory backend: {error}");
        }
        return None;
    };

    let mut item = HistoryItem::from_command_line(code);
    item.start_timestamp = Some(chrono::Utc::now());
    item.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
    item.cwd = std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    item.session_id = session_id;

    match store.save_known(item, HistoryExtraInfo::default()) {
        Ok(item) => item.id,
        Err(error) => {
            log::warn!("Failed to save IPC history: {error}");
            None
        }
    }
}

/// Finalize metadata for the row identified by an editor save outcome.
///
/// A failed or absent outcome leaves the row's metadata as SQL NULL, which
/// means "arf does not know".  Writing `false` would claim that arf knows the
/// row was an ordinary command when the adapter did not save one successfully.
pub(super) fn finalize_history(
    handle: Option<&HistoryHandle>,
    outcome: Option<HistorySaveOutcome>,
    is_meta_command: bool,
) {
    let Some(handle) = handle else {
        return;
    };
    let Some(HistorySaveOutcome::Saved(id)) = outcome else {
        return;
    };

    if let Err(error) = handle.store.finalize_meta_command(id, is_meta_command) {
        log::warn!("Failed to finalize history metadata for {id}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RSourceStatus;
    use crate::editor::prompt::PromptFormatter;
    use crate::history::HistorySaveReceipt;
    use crate::repl::meta_command::process_meta_command;
    use crate::repl::state::{PendingHistoryContext, PromptRuntimeConfig};
    use reedline::{
        FileBackedHistory, History, HistoryItem, SearchDirection, SearchQuery, SqliteBackedHistory,
    };

    fn everything_query() -> SearchQuery {
        SearchQuery::everything(SearchDirection::Forward, None)
    }

    #[test]
    fn save_ipc_history_ignores_whitespace_only_code() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let store = HistoryStore::open(temp_dir.path().join("r.db"), None, None).unwrap();

        let mut sink = FileBackedHistory::default();
        assert!(save_ipc_history(&mut sink, Some(store.clone()), r"   ", None).is_none());
        assert_eq!(store.count(everything_query()).unwrap(), 0);
    }

    #[test]
    fn save_ipc_history_writes_empty_metadata_and_returns_id() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let store = HistoryStore::open(temp_dir.path().join("r.db"), None, None).unwrap();

        let mut sink = FileBackedHistory::default();
        let id = save_ipc_history(&mut sink, Some(store), r#":starts as R code"#, None).unwrap();
        let stored = SqliteBackedHistory::with_file(temp_dir.path().join("r.db"), None, None)
            .unwrap()
            .load_with_extra::<HistoryExtraInfo>(id)
            .unwrap();
        assert_eq!(stored.more_info, Some(HistoryExtraInfo::default()));
    }

    /// reedline saves a whitespace-only buffer, so finalization must reach it.
    /// Trimming before the emptiness test would leave the row NULL, which means
    /// "arf does not know" — a claim that is false for a line arf just routed.
    #[test]
    fn whitespace_only_line_is_finalized_and_unsaved_followup_is_ignored() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let path = temp_dir.path().join("r.db");
        let store = HistoryStore::open(path.clone(), None, None).unwrap();
        let receipt = HistorySaveReceipt::new();
        let handle = HistoryHandle {
            store: store.clone(),
            receipt: receipt.clone(),
        };

        let mut adapter = ReedlineHistoryAdapter::new(store, receipt.clone());
        let saved = adapter
            .save(HistoryItem::from_command_line(r"   "))
            .unwrap();
        let id = saved.id.unwrap();

        finalize_history(Some(&handle), receipt.take(), false);
        let reopened = SqliteBackedHistory::with_file(path.clone(), None, None).unwrap();
        assert_eq!(
            reopened
                .load_with_extra::<HistoryExtraInfo>(id)
                .unwrap()
                .more_info,
            Some(HistoryExtraInfo::default()),
        );

        // An empty line is never saved by reedline, so its empty receipt
        // outcome must not re-finalize the previous row.
        finalize_history(Some(&handle), receipt.take(), true);
        let reopened = SqliteBackedHistory::with_file(path, None, None).unwrap();
        assert_eq!(
            reopened
                .load_with_extra::<HistoryExtraInfo>(id)
                .unwrap()
                .more_info,
            Some(HistoryExtraInfo::default()),
        );
    }

    #[test]
    fn taken_outcome_can_finalize_and_update_exit_status() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let store = HistoryStore::open(temp_dir.path().join("r.db"), None, None).unwrap();
        let receipt = HistorySaveReceipt::new();
        let handle = HistoryHandle {
            store: store.clone(),
            receipt: receipt.clone(),
        };
        let mut adapter = ReedlineHistoryAdapter::new(store.clone(), receipt);
        let id = adapter
            .save(HistoryItem::from_command_line(r"ordinary command"))
            .unwrap()
            .id
            .unwrap();
        let outcome = handle.receipt.take();

        // The same taken value is passed to both consumers, as it is on the
        // ordinary top-level R path: finalization and pending exit status.
        finalize_history(Some(&handle), outcome, false);
        let pending_history_context = PendingHistoryContext::Command {
            store: Some(store.clone()),
            history_id: match outcome {
                Some(HistorySaveOutcome::Saved(id)) => Some(id),
                _ => None,
            },
        };
        if let PendingHistoryContext::Command {
            store: Some(store),
            history_id: Some(id),
        } = pending_history_context
        {
            store.set_exit_status(id, 0).unwrap();
        }

        let stored = store.load_with_metadata(id).unwrap();
        assert_eq!(stored.more_info, Some(HistoryExtraInfo::default()));
        assert_eq!(stored.exit_status, Some(0));
    }

    /// Without a persistent store, reedline keeps an in-memory backend that
    /// ordinary typed input still reaches, so IPC code has to reach it too --
    /// otherwise injected commands vanish from in-session recall under
    /// `--no-history` or when the database fails to open.
    #[test]
    fn ipc_history_falls_back_to_the_in_memory_backend_without_a_store() {
        let mut memory = FileBackedHistory::default();

        assert!(save_ipc_history(&mut memory, None, r#"print("hi")"#, None).is_none());

        let stored = memory.search(everything_query()).unwrap();
        assert_eq!(
            stored
                .iter()
                .map(|item| item.command_line.as_str())
                .collect::<Vec<_>>(),
            vec![r#"print("hi")"#],
        );
    }

    /// With a store the adapter owns persistence, so the editor backend must be
    /// left alone; writing to both would insert the row into SQLite twice.
    #[test]
    fn ipc_history_does_not_also_write_to_the_editor_backend_with_a_store() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let store = HistoryStore::open(temp_dir.path().join("r.db"), None, None).unwrap();
        let mut memory = FileBackedHistory::default();

        save_ipc_history(&mut memory, Some(store.clone()), r#"print("hi")"#, None).unwrap();

        assert_eq!(store.count(everything_query()).unwrap(), 1);
        assert!(memory.search(everything_query()).unwrap().is_empty());
    }

    #[test]
    fn finalization_without_a_store_is_a_no_op() {
        let mut sink = FileBackedHistory::default();
        assert!(save_ipc_history(&mut sink, None, "x <- 1", None).is_none());
        finalize_history(None, None, false);
    }

    #[test]
    fn finalization_follows_dispatch_result_for_all_routing_shapes() {
        fn was_dispatched(input: &str) -> bool {
            let mut prompt = PromptRuntimeConfig::builder(
                PromptFormatter::default(),
                "r> ",
                "+  ",
                "[shell] $ ",
            )
            .build();
            let mut dir_stack = Vec::new();
            process_meta_command(
                input,
                &mut prompt,
                &None,
                &None,
                &RSourceStatus::Path,
                &mut dir_stack,
                None,
                None,
            )
            .is_some()
        }

        let temp_dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(temp_dir.path().join("r.db"), None, None).unwrap();

        for command in [":help", ":user_defined"] {
            let item = store
                .save_unknown(HistoryItem::from_command_line(command))
                .unwrap();
            store
                .finalize_meta_command(item.id.unwrap(), was_dispatched(command))
                .unwrap();
            assert_eq!(
                store
                    .load_with_metadata(item.id.unwrap())
                    .unwrap()
                    .more_info
                    .unwrap()
                    .meta_command(),
                Some(true)
            );
        }

        for command in [r"x <- 1:10", "utils::head(x)"] {
            let item = store
                .save_unknown(HistoryItem::from_command_line(command))
                .unwrap();
            store
                .finalize_meta_command(item.id.unwrap(), was_dispatched(command))
                .unwrap();
            assert_eq!(
                store
                    .load_with_metadata(item.id.unwrap())
                    .unwrap()
                    .more_info
                    .unwrap()
                    .meta_command(),
                None
            );
        }

        // A menu/continuation caller decides whether to dispatch independently
        // of the text.  The finalizer receives that decision, never a prefix.
        let menu = store
            .save_unknown(HistoryItem::from_command_line(r#":menu choice"#))
            .unwrap();
        store
            .finalize_meta_command(menu.id.unwrap(), false)
            .unwrap();
        assert_eq!(
            store
                .load_with_metadata(menu.id.unwrap())
                .unwrap()
                .more_info
                .unwrap()
                .meta_command(),
            None
        );

        assert!(!was_dispatched("echo ordinary shell command"));
        assert!(was_dispatched(":r"));
    }
}
