//! REPL history setup and persistence helpers.

use crate::history::{
    HistoryExtraInfo, HistoryHandle, HistorySaveOutcome, HistoryStore, ReedlineHistoryAdapter,
};
use reedline::{HistoryItem, HistoryItemId, HistorySessionId, Reedline};

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
pub(super) fn save_ipc_history(
    store: Option<&HistoryStore>,
    code: &str,
    session_id: Option<HistorySessionId>,
) -> Option<HistoryItemId> {
    if code.trim().is_empty() {
        return None;
    }

    let store = store?;

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

/// Finalize metadata for the row an editor adapter saved for `line`.
///
/// reedline saves every buffer it does not consider empty, testing the raw
/// buffer rather than a trimmed one, so mirror that exact condition here: a
/// whitespace-only submission is still a history row and still needs its
/// metadata written. Skipping it would leave the row NULL, which means "arf
/// does not know" — and arf does know such a line is not a meta command.
pub(super) fn finalize_history(handle: Option<&HistoryHandle>, line: &str, is_meta_command: bool) {
    let Some(handle) = handle else {
        return;
    };
    if line.is_empty() {
        return;
    }
    let Some(HistorySaveOutcome::Saved(id)) = handle.receipt.latest() else {
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
    use crate::repl::state::PromptRuntimeConfig;
    use reedline::{History, HistoryItem, SearchDirection, SearchQuery, SqliteBackedHistory};

    fn everything_query() -> SearchQuery {
        SearchQuery::everything(SearchDirection::Forward, None)
    }

    #[test]
    fn save_ipc_history_ignores_whitespace_only_code() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let store = HistoryStore::open(temp_dir.path().join("r.db"), None, None).unwrap();

        assert!(save_ipc_history(Some(&store), r"   ", None).is_none());
        assert_eq!(store.count(everything_query()).unwrap(), 0);
    }

    #[test]
    fn save_ipc_history_writes_empty_metadata_and_returns_id() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let store = HistoryStore::open(temp_dir.path().join("r.db"), None, None).unwrap();

        let id = save_ipc_history(Some(&store), r#":starts as R code"#, None).unwrap();
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
    fn whitespace_only_line_is_finalized_but_an_empty_line_is_not() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let path = temp_dir.path().join("r.db");
        let store = HistoryStore::open(path.clone(), None, None).unwrap();
        let receipt = HistorySaveReceipt::new();
        let handle = HistoryHandle {
            store: store.clone(),
            receipt: receipt.clone(),
        };

        let mut adapter = ReedlineHistoryAdapter::new(store, receipt);
        let saved = adapter
            .save(HistoryItem::from_command_line(r"   "))
            .unwrap();
        let id = saved.id.unwrap();

        // An empty line is never saved by reedline, so it must not touch the
        // row the receipt still refers to.
        finalize_history(Some(&handle), "", false);
        let reopened = SqliteBackedHistory::with_file(path.clone(), None, None).unwrap();
        assert_eq!(
            reopened
                .load_with_extra::<HistoryExtraInfo>(id)
                .unwrap()
                .more_info,
            None,
        );

        finalize_history(Some(&handle), r"   ", false);
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
    fn no_store_is_a_no_op() {
        assert!(save_ipc_history(None, "x <- 1", None).is_none());
        finalize_history(None, "x <- 1", false);
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
