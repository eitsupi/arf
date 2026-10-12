//! Filter query parsing and application for the history browser.

use super::HistoryBrowser;
use crate::fuzzy::fuzzy_match;

/// Parsed filter with optional prefix filters.
#[derive(Debug, Default)]
pub(super) struct HistoryFilter {
    /// Raw query string (for display).
    pub(super) raw_query: String,
    /// Cursor position in the query string.
    pub(super) cursor_pos: usize,
    /// Hostname filter (from `host:xxx`).
    pub(super) hostname: Option<String>,
    /// CWD prefix filter (from `cwd:/path`).
    pub(super) cwd_prefix: Option<String>,
    /// Exit status filter (from `exit:N`).
    pub(super) exit_status: Option<i64>,
    /// Command pattern for fuzzy search (remaining text after prefix filters).
    pub(super) command_pattern: String,
}

impl HistoryFilter {
    /// Parse a query string into filter components.
    pub(super) fn parse(query: &str) -> Self {
        let mut filter = HistoryFilter {
            raw_query: query.to_string(),
            cursor_pos: query.chars().count(),
            ..Default::default()
        };

        let mut remaining_parts = Vec::new();

        for part in query.split_whitespace() {
            if let Some(hostname) = part.strip_prefix("host:") {
                filter.hostname = Some(hostname.to_string());
            } else if let Some(cwd) = part.strip_prefix("cwd:") {
                filter.cwd_prefix = Some(cwd.to_string());
            } else if let Some(status) = part.strip_prefix("exit:") {
                if let Ok(n) = status.parse::<i64>() {
                    filter.exit_status = Some(n);
                } else {
                    remaining_parts.push(part);
                }
            } else {
                remaining_parts.push(part);
            }
        }

        filter.command_pattern = remaining_parts.join(" ");
        filter
    }

    /// Re-parse prefix filters from the current raw_query.
    /// Call this after modifying `raw_query` or `cursor_pos` in-place.
    pub(super) fn reparse(&mut self) {
        let parsed = Self::parse(&self.raw_query);
        self.hostname = parsed.hostname;
        self.cwd_prefix = parsed.cwd_prefix;
        self.exit_status = parsed.exit_status;
        self.command_pattern = parsed.command_pattern;
    }
}

impl HistoryBrowser {
    /// Update the filtered list based on the current filter.
    pub(super) fn update_filter(&mut self) {
        if self.filter.command_pattern.is_empty()
            && self.filter.hostname.is_none()
            && self.filter.cwd_prefix.is_none()
            && self.filter.exit_status.is_none()
        {
            // No filter - show all entries
            self.filtered = self
                .entries
                .iter()
                .enumerate()
                .map(|(i, _)| (i, 0))
                .collect();
        } else {
            let mut results: Vec<(usize, u32)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, entry)| {
                    // Apply hostname filter
                    if let Some(ref hostname) = self.filter.hostname {
                        let item_host = entry.item.hostname.as_deref()?;
                        if !item_host.contains(hostname) {
                            return None;
                        }
                    }

                    // Apply cwd prefix filter
                    if let Some(ref cwd_prefix) = self.filter.cwd_prefix {
                        let item_cwd = entry.item.cwd.as_deref()?;
                        if !item_cwd.starts_with(cwd_prefix) {
                            return None;
                        }
                    }

                    // Apply exit status filter
                    if let Some(exit_status) = self.filter.exit_status
                        && entry.item.exit_status != Some(exit_status)
                    {
                        return None;
                    }

                    // Apply fuzzy command pattern filter
                    if !self.filter.command_pattern.is_empty() {
                        if let Some(m) =
                            fuzzy_match(&self.filter.command_pattern, &entry.item.command_line)
                        {
                            return Some((idx, m.score));
                        }
                        return None;
                    }

                    Some((idx, 0))
                })
                .collect();

            // Sort by score (descending) if we have fuzzy scores
            if !self.filter.command_pattern.is_empty() {
                results.sort_by_key(|entry| std::cmp::Reverse(entry.1));
            }

            self.filtered = results;
        }

        // Reset cursor and scroll
        self.cursor = 0;
        self.scroll_offset = 0;
    }
}
