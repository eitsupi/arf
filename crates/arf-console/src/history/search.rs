//! Fuzzy history search wrapper for reedline.
//!
//! This module provides a `FuzzyHistory` wrapper that enhances reedline's
//! history search with fuzzy matching capabilities using the nucleo library.

use crate::fuzzy::fuzzy_match;
use crate::history::HistoryExtraInfo;
use reedline::{
    History, HistoryItem, HistoryItemExtraInfo, HistoryItemId, HistorySessionId,
    IgnoreAllExtraInfo, Result, SearchFilter, SearchQuery, SqliteBackedHistory,
};

/// Returns whether a line is a meta command according to the REPL's syntax.
fn is_meta_command(line: &str) -> bool {
    line.trim_start().starts_with(':')
}

fn convert_history_item<A: HistoryItemExtraInfo, B: HistoryItemExtraInfo>(
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

/// A wrapper around `SqliteBackedHistory` that provides fuzzy search capabilities.
///
/// When performing a substring search (Ctrl+R history search), this wrapper
/// applies fuzzy matching instead of exact substring matching, providing
/// fzf-style search experience.
pub struct FuzzyHistory {
    inner: SqliteBackedHistory,
    /// Whether fuzzy search is enabled. If false, delegates directly to inner.
    fuzzy_enabled: bool,
}

impl FuzzyHistory {
    /// Create a new FuzzyHistory wrapper around a SqliteBackedHistory.
    pub fn new(inner: SqliteBackedHistory) -> Self {
        Self {
            inner,
            fuzzy_enabled: true,
        }
    }

    /// Enable or disable fuzzy search.
    #[allow(dead_code)]
    pub fn set_fuzzy_enabled(&mut self, enabled: bool) {
        self.fuzzy_enabled = enabled;
    }

    /// Check if fuzzy search is enabled.
    #[allow(dead_code)]
    pub fn is_fuzzy_enabled(&self) -> bool {
        self.fuzzy_enabled
    }

    /// Perform fuzzy search on history items.
    ///
    /// Gets all matching items from inner history and applies fuzzy matching,
    /// returning results sorted by fuzzy match score.
    fn fuzzy_search(&self, query: SearchQuery, pattern: &str) -> Result<Vec<HistoryItem>> {
        // Create a filter that preserves session and other public fields but removes command_line
        // We can't use struct update syntax because not_command_line is pub(crate)
        let mut filter = SearchFilter::anything(query.filter.session);
        filter.hostname = query.filter.hostname.clone();
        filter.cwd_exact = query.filter.cwd_exact.clone();
        filter.cwd_prefix = query.filter.cwd_prefix.clone();
        filter.exit_successful = query.filter.exit_successful;
        // command_line is intentionally left as None - we'll do fuzzy matching

        // Get all items without command line filter
        let modified_query = SearchQuery {
            direction: query.direction,
            start_time: query.start_time,
            end_time: query.end_time,
            start_id: query.start_id,
            end_id: query.end_id,
            limit: Some(1000), // Limit the initial fetch to a reasonable number
            filter,
        };

        // Get candidates from inner history
        let candidates = self.inner.search(modified_query)?;

        // Apply fuzzy matching
        let mut scored: Vec<(HistoryItem, u32)> = candidates
            .into_iter()
            .filter_map(|item| fuzzy_match(pattern, &item.command_line).map(|m| (item, m.score)))
            .collect();

        // Sort by score (descending)
        scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));

        // Apply original limit if specified
        let results: Vec<HistoryItem> = if let Some(limit) = query.limit {
            scored
                .into_iter()
                .take(limit as usize)
                .map(|(item, _)| item)
                .collect()
        } else {
            scored.into_iter().map(|(item, _)| item).collect()
        };

        Ok(results)
    }
}

impl History for FuzzyHistory {
    fn save(&mut self, mut h: HistoryItem) -> Result<HistoryItem> {
        // Populate metadata if not already set
        if h.start_timestamp.is_none() {
            h.start_timestamp = Some(chrono::Utc::now());
        }
        if h.cwd.is_none() {
            h.cwd = std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned());
        }
        if h.hostname.is_none() {
            h.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
        }

        let meta_command = is_meta_command(&h.command_line);
        let h = convert_history_item(
            h,
            Some(HistoryExtraInfo {
                meta_command,
                ..Default::default()
            }),
        );
        let saved = self.inner.save_with_extra(h)?;
        Ok(convert_history_item::<HistoryExtraInfo, IgnoreAllExtraInfo>(saved, None))
    }

    fn load(&self, id: HistoryItemId) -> Result<HistoryItem> {
        self.inner.load(id)
    }

    fn count(&self, query: SearchQuery) -> Result<i64> {
        self.inner.count(query)
    }

    fn search(&self, query: SearchQuery) -> Result<Vec<HistoryItem>> {
        // Check if this is a substring search that we should make fuzzy
        if self.fuzzy_enabled
            && let Some(ref cmd_search) = query.filter.command_line
        {
            // Check if it's a Substring search (used by Ctrl+R)
            if let reedline::CommandLineSearch::Substring(pattern) = cmd_search
                && !pattern.is_empty()
            {
                let pattern = pattern.clone();
                return self.fuzzy_search(query, &pattern);
            }
        }

        // Delegate to inner for non-fuzzy searches
        self.inner.search(query)
    }

    fn update(
        &mut self,
        id: HistoryItemId,
        updater: &dyn Fn(HistoryItem) -> HistoryItem,
    ) -> Result<()> {
        self.inner.update(id, updater)
    }

    fn clear(&mut self) -> Result<()> {
        self.inner.clear()
    }

    fn delete(&mut self, h: HistoryItemId) -> Result<()> {
        self.inner.delete(h)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.inner.sync()
    }

    fn session(&self) -> Option<HistorySessionId> {
        self.inner.session()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::History;

    #[test]
    fn meta_command_detection_matches_repl_syntax() {
        let cases = [
            (":cd", true),
            ("  :help", true),
            ("x <- 1 + 1", false),
            ("x <- 1:10", false),
            ("utils::head(x)", false),
            ("", false),
        ];

        for (line, expected) in cases {
            assert_eq!(is_meta_command(line), expected, "line: {line:?}");
        }
    }

    #[test]
    fn save_populates_meta_command_metadata() {
        let temp_dir = tempfile::tempdir().expect("create temporary history directory");
        let history_path = temp_dir.path().join("history.db");
        let inner = SqliteBackedHistory::with_file(history_path.clone(), None, None)
            .expect("create SQLite history");
        let mut history = FuzzyHistory::new(inner);

        let meta_id = history
            .save(HistoryItem::from_command_line(":cd /tmp"))
            .expect("save meta command")
            .id
            .expect("saved meta command should have an ID");
        let normal_id = history
            .save(HistoryItem::from_command_line("x <- 1:10"))
            .expect("save normal command")
            .id
            .expect("saved normal command should have an ID");
        drop(history);

        let stored = SqliteBackedHistory::with_file(history_path, None, None)
            .expect("reopen SQLite history");
        let meta = stored
            .load_with_extra::<HistoryExtraInfo>(meta_id)
            .expect("load meta command");
        let normal = stored
            .load_with_extra::<HistoryExtraInfo>(normal_id)
            .expect("load normal command");

        assert!(meta.more_info.expect("meta metadata").meta_command);
        assert!(!normal.more_info.expect("normal metadata").meta_command);
    }
}
