//! Typed history queries with strict session scoping.

use super::store::HistoryStore;
use chrono::{DateTime, NaiveDate, Utc};
use reedline::{
    CommandLineSearch, HistoryItem, Result, SearchDirection, SearchFilter, SearchQuery,
};

/// Error returned when a `since` bound is neither an RFC 3339 datetime nor a
/// `YYYY-MM-DD` date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InvalidSince;

/// Parse a `since` bound given as an RFC 3339 datetime or a `YYYY-MM-DD` date
/// (interpreted as midnight UTC), truncated to millisecond precision.
pub(crate) fn parse_since(since: &str) -> std::result::Result<DateTime<Utc>, InvalidSince> {
    let parsed = DateTime::parse_from_rfc3339(since)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDate::parse_from_str(since, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
        })
        .map_err(|_| InvalidSince)?;
    DateTime::from_timestamp_millis(parsed.timestamp_millis()).ok_or(InvalidSince)
}

/// Filters for [`HistoryStore::query`].
pub(crate) struct HistoryQuery {
    /// Maximum number of rows to return.
    pub limit: i64,
    /// Include rows from every session instead of only the store's session.
    pub all_sessions: bool,
    /// Only rows recorded in exactly this working directory.
    pub cwd: Option<String>,
    /// Only rows whose command line contains this substring.
    pub grep: Option<String>,
    /// Only rows started at or after this time.
    pub since: Option<DateTime<Utc>>,
}

impl HistoryStore {
    /// Return the newest rows matching `query`, newest first.
    ///
    /// Session scoping is strict: unless `all_sessions` is set, only rows of
    /// this store's session are returned.
    pub(crate) fn query(&self, query: &HistoryQuery) -> Result<Vec<HistoryItem>> {
        // Do not pass the session to reedline's filter: its persistent semantics
        // intentionally include rows from before the current session. The owned
        // store applies strict session matching over bounded pages and applies the
        // limit only after matching rows are collected.
        self.search_strict_session(
            |start_id| {
                let mut filter = SearchFilter::anything(None);
                filter.cwd_exact = query.cwd.clone();
                filter.command_line = query.grep.clone().map(CommandLineSearch::Substring);
                SearchQuery {
                    direction: SearchDirection::Backward,
                    // reedline 0.50's start_time SQL uses a misspelled column.
                    // The owned store applies this bound after each page.
                    start_time: None,
                    end_time: None,
                    start_id,
                    end_id: None,
                    limit: None,
                    filter,
                }
            },
            self.session(),
            query.all_sessions,
            query.limit,
            query.since,
        )
    }
}
