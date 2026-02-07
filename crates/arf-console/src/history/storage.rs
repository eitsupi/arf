//! History types for SQLite-backed command history.
//!
//! This module provides types for arf-console history management.
//! `HistoryMode` implements reedline's `HistoryItemExtraInfo` trait and can be
//! stored in the `more_info` field of `HistoryItem<HistoryMode>`.
//!
//! Note: reedline's `History` trait currently uses `HistoryItem<IgnoreAllExtraInfo>`
//! (non-generic), so storing custom `more_info` through the trait API is not yet
//! possible. The trait implementation is prepared here for when the `History` trait
//! becomes generic over `ExtraInfo`.
//!
//! TODO: Once reedline's `History` trait supports generic `ExtraInfo`, update
//! `FuzzyHistory` to save/load `HistoryItem<HistoryMode>` and populate the
//! `more_info` field on save (R mode vs Shell mode vs Reprex mode).

use reedline::HistoryItemExtraInfo;
use serde::{Deserialize, Serialize};

/// Mode in which a command was executed.
///
/// Implements [`HistoryItemExtraInfo`] so it can be used as the `more_info`
/// field of a `HistoryItem<HistoryMode>`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryMode {
    /// Normal R command.
    #[default]
    R,
    /// Shell mode command.
    Shell,
    /// Reprex mode command.
    Reprex,
}

impl HistoryItemExtraInfo for HistoryMode {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_mode_variants() {
        let r = HistoryMode::R;
        let shell = HistoryMode::Shell;
        let reprex = HistoryMode::Reprex;

        assert_ne!(r, shell);
        assert_ne!(shell, reprex);
        assert_ne!(r, reprex);
    }

    #[test]
    fn test_history_mode_clone() {
        let mode = HistoryMode::Shell;
        let cloned = mode;
        assert_eq!(mode, cloned);
    }

    #[test]
    fn test_history_mode_default() {
        assert_eq!(HistoryMode::default(), HistoryMode::R);
    }

    #[test]
    fn test_history_mode_serialization() {
        let mode = HistoryMode::Shell;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"Shell\"");

        let deserialized: HistoryMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, mode);
    }

    #[test]
    fn test_history_mode_as_extra_info() {
        use reedline::HistoryItem;

        // Verify HistoryMode can be used as HistoryItem's ExtraInfo type parameter
        let item: HistoryItem<HistoryMode> = HistoryItem {
            id: None,
            start_timestamp: None,
            command_line: "1 + 1".to_string(),
            session_id: None,
            hostname: None,
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: Some(HistoryMode::R),
        };

        assert_eq!(item.more_info, Some(HistoryMode::R));

        // Verify round-trip serialization with HistoryItem
        let json = serde_json::to_string(&item).unwrap();
        let deserialized: HistoryItem<HistoryMode> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.more_info, Some(HistoryMode::R));
        assert_eq!(deserialized.command_line, "1 + 1");
    }
}
