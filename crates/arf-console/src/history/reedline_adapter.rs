//! Reedline adapter and fuzzy Ctrl+R history search.
//!
//! The adapter deliberately saves interactive entries with unknown metadata.
//! Routing code finalizes that metadata after it knows whether the line was
//! dispatched as an arf meta command.

use super::store::{HistorySaveOutcome, HistorySaveReceipt, HistoryStore};
use crate::fuzzy::fuzzy_match;
use reedline::{
    History, HistoryItem, HistoryItemId, HistorySessionId, Result, SearchFilter, SearchQuery,
};

/// A reedline-compatible view over an arf-owned history store.
pub struct ReedlineHistoryAdapter {
    store: HistoryStore,
    receipt: HistorySaveReceipt,
    fuzzy_enabled: bool,
}

impl ReedlineHistoryAdapter {
    pub fn new(store: HistoryStore, receipt: HistorySaveReceipt) -> Self {
        Self {
            store,
            receipt,
            fuzzy_enabled: true,
        }
    }

    fn fuzzy_search(&self, query: SearchQuery, pattern: &str) -> Result<Vec<HistoryItem>> {
        let mut filter = SearchFilter::anything(query.filter.session);
        filter.hostname = query.filter.hostname.clone();
        filter.cwd_exact = query.filter.cwd_exact.clone();
        filter.cwd_prefix = query.filter.cwd_prefix.clone();
        filter.exit_successful = query.filter.exit_successful;

        let modified_query = SearchQuery {
            direction: query.direction,
            start_time: query.start_time,
            end_time: query.end_time,
            start_id: query.start_id,
            end_id: query.end_id,
            limit: Some(1000),
            filter,
        };

        let candidates = self.store.search(modified_query)?;
        let mut scored: Vec<(HistoryItem, u32)> = candidates
            .into_iter()
            .filter_map(|item| fuzzy_match(pattern, &item.command_line).map(|m| (item, m.score)))
            .collect();
        scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));

        Ok(match query.limit {
            Some(limit) => scored
                .into_iter()
                .take(limit as usize)
                .map(|(item, _)| item)
                .collect(),
            None => scored.into_iter().map(|(item, _)| item).collect(),
        })
    }
}

impl History for ReedlineHistoryAdapter {
    fn save(&mut self, mut item: HistoryItem) -> Result<HistoryItem> {
        if item.start_timestamp.is_none() {
            item.start_timestamp = Some(chrono::Utc::now());
        }
        if item.cwd.is_none() {
            item.cwd = std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned());
        }
        if item.hostname.is_none() {
            item.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
        }

        match self.store.save_unknown(item.clone()) {
            Ok(saved) => {
                if let Some(id) = saved.id {
                    self.receipt.record(HistorySaveOutcome::Saved(id));
                    Ok(saved)
                } else {
                    log::warn!("History save returned no row ID");
                    self.receipt.record(HistorySaveOutcome::Failed);
                    Ok(item)
                }
            }
            Err(error) => {
                log::warn!("Failed to save history: {error}");
                self.receipt.record(HistorySaveOutcome::Failed);
                item.id = None;
                Ok(item)
            }
        }
    }

    fn load(&self, id: HistoryItemId) -> Result<HistoryItem> {
        self.store.load(id)
    }

    fn count(&self, query: SearchQuery) -> Result<i64> {
        self.store.count(query)
    }

    fn search(&self, query: SearchQuery) -> Result<Vec<HistoryItem>> {
        if self.fuzzy_enabled
            && let Some(reedline::CommandLineSearch::Substring(pattern)) =
                query.filter.command_line.as_ref()
            && !pattern.is_empty()
        {
            let pattern = pattern.clone();
            return self.fuzzy_search(query, &pattern);
        }
        self.store.search(query)
    }

    fn update(
        &mut self,
        id: HistoryItemId,
        updater: &dyn Fn(HistoryItem) -> HistoryItem,
    ) -> Result<()> {
        self.store.update(id, updater)
    }

    fn clear(&mut self) -> Result<()> {
        self.store.clear()
    }

    fn delete(&mut self, id: HistoryItemId) -> Result<()> {
        self.store.delete(id)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.store.sync()
    }

    fn session(&self) -> Option<HistorySessionId> {
        self.store.session()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistoryExtraInfo, HistorySaveReceipt};
    use reedline::History;

    #[test]
    fn dispatched_colon_lines_are_flagged_but_known_routing_is_not_lexical() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path().join("history.db"), None, None).unwrap();
        let receipt = HistorySaveReceipt::new();
        let mut adapter = ReedlineHistoryAdapter::new(store.clone(), receipt);

        let known_meta = adapter
            .save(HistoryItem::from_command_line(":help"))
            .unwrap();
        let known_meta_id = known_meta.id.unwrap();
        store.finalize_meta_command(known_meta_id, true).unwrap();

        let unknown_meta = adapter
            .save(HistoryItem::from_command_line(":user_defined"))
            .unwrap();
        let unknown_meta_id = unknown_meta.id.unwrap();
        store.finalize_meta_command(unknown_meta_id, true).unwrap();

        let known = store.load_with_metadata(known_meta_id).unwrap();
        let unknown = store.load_with_metadata(unknown_meta_id).unwrap();
        assert_eq!(known.more_info.unwrap().meta_command(), Some(true));
        assert_eq!(unknown.more_info.unwrap().meta_command(), Some(true));
    }

    #[test]
    fn menu_and_ipc_colon_lines_are_ordinary() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path().join("history.db"), None, None).unwrap();

        let menu = store
            .save_unknown(HistoryItem::from_command_line(r#":menu choice"#))
            .unwrap();
        store
            .finalize_meta_command(menu.id.unwrap(), false)
            .unwrap();

        let ipc = store
            .save_known(
                HistoryItem::from_command_line(r#":ipc code"#),
                HistoryExtraInfo::default(),
            )
            .unwrap();

        let menu_info = store.load_with_metadata(menu.id.unwrap()).unwrap();
        let ipc_info = store.load_with_metadata(ipc.id.unwrap()).unwrap();
        assert_eq!(menu_info.more_info.unwrap().meta_command(), None);
        assert_eq!(ipc_info.more_info.unwrap().meta_command(), None);
    }

    #[test]
    fn save_failure_is_reported_without_propagating_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let store = HistoryStore::open(path.clone(), None, None).unwrap();
        let receipt = HistorySaveReceipt::new();
        let mut adapter = ReedlineHistoryAdapter::new(store.clone(), receipt.clone());
        store.drop_table_for_test(&path);

        let saved = adapter
            .save(HistoryItem::from_command_line("save failure"))
            .unwrap();
        assert_eq!(saved.id, None);
        assert_eq!(receipt.latest(), Some(HistorySaveOutcome::Failed));
    }
}
