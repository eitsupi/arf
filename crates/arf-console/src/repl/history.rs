//! REPL history setup and IPC history persistence.

use crate::history::FuzzyHistory;
use reedline::{HistorySessionId, Reedline, SqliteBackedHistory};

/// Set up history for a line editor with a specific database path.
///
/// Returns `(editor, true)` if history was successfully configured, or
/// `(editor, false)` if history path was `None` or the database failed to open.
/// The history is wrapped with FuzzyHistory to provide fuzzy search capabilities.
pub(super) fn setup_history(
    line_editor: Reedline,
    history_path: Option<std::path::PathBuf>,
    session_id: Option<HistorySessionId>,
) -> (Reedline, bool) {
    let Some(path) = history_path else {
        return (line_editor, false);
    };
    match SqliteBackedHistory::with_file(path.clone(), session_id, Some(chrono::Utc::now())) {
        Ok(history) => {
            let fuzzy_history = FuzzyHistory::new(history);
            let editor = line_editor
                .with_history_session_id(session_id)
                .with_history(Box::new(fuzzy_history));
            (editor, true)
        }
        Err(e) => {
            log::warn!("Failed to open history database {}: {}", path.display(), e);
            (line_editor, false)
        }
    }
}

/// Save an IPC-injected command using the same metadata as headless history.
///
/// A history failure is deliberately non-fatal: the injected code must still
/// be evaluated and its IPC response must still be delivered.
pub(super) fn save_ipc_history(
    editor: &mut Reedline,
    code: &str,
    session_id: Option<HistorySessionId>,
) -> Option<reedline::HistoryItemId> {
    if code.trim().is_empty() {
        return None;
    }

    let mut item = reedline::HistoryItem::from_command_line(code);
    item.start_timestamp = Some(chrono::Utc::now());
    item.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
    item.cwd = std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    item.session_id = session_id;

    match editor.history_mut().save(item) {
        Ok(item) => item.id,
        Err(e) => {
            log::warn!("Failed to save IPC history: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod ipc_history_tests {
    use super::save_ipc_history;
    use reedline::{Reedline, SearchDirection, SearchQuery, SqliteBackedHistory};

    fn everything_query() -> SearchQuery {
        SearchQuery::everything(SearchDirection::Forward, None)
    }

    #[test]
    fn save_ipc_history_ignores_whitespace_only_code() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let history = SqliteBackedHistory::with_file(temp_dir.path().join("r.db"), None, None)
            .expect("create SQLite history");
        let mut editor = Reedline::create().with_history(Box::new(history));

        assert!(save_ipc_history(&mut editor, " \t\n ", None).is_none());
        assert_eq!(
            editor
                .history()
                .count(everything_query())
                .expect("count history entries"),
            0
        );
    }

    #[test]
    fn save_ipc_history_returns_id_for_later_status_update() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let history = SqliteBackedHistory::with_file(temp_dir.path().join("r.db"), None, None)
            .expect("create SQLite history");
        let mut editor = Reedline::create().with_history(Box::new(history));

        let id = save_ipc_history(&mut editor, "ipc_history_test <- 1", None)
            .expect("saved IPC history should have an ID");
        editor
            .history_mut()
            .update(id, &|mut item| {
                item.exit_status = Some(1);
                item
            })
            .expect("saved IPC history should be updateable by ID");

        let item = editor.history().load(id).expect("load saved IPC history");
        assert_eq!(item.command_line, "ipc_history_test <- 1");
        assert_eq!(item.exit_status, Some(1));
    }
}
