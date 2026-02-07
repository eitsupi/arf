//! Fuzzy history search wrapper for reedline.
//!
//! This module provides a `FuzzyHistory` wrapper that enhances reedline's
//! history search with fuzzy matching capabilities using the nucleo library.

use crate::fuzzy::fuzzy_match;
use reedline::{
    History, HistoryItem, HistoryItemId, HistorySessionId, Result, SearchFilter, SearchQuery,
    SqliteBackedHistory,
};

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
        scored.sort_by(|a, b| b.1.cmp(&a.1));

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
        // TODO: Once reedline's History trait accepts HistoryItem<HistoryMode>,
        // populate h.more_info with the current execution mode (R/Shell/Reprex).
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
        self.inner.save(h)
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
    // Note: These tests require a working SQLite history which is tested
    // in the integration tests. Unit tests here focus on the wrapper logic.

    #[test]
    fn test_fuzzy_history_module_compiles() {
        // We can't create a SqliteBackedHistory without a file in unit tests,
        // so this test just verifies the module compiles correctly.
        // Integration tests will cover actual functionality.
    }
}
