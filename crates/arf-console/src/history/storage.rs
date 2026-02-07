//! History types for SQLite-backed command history.
//!
//! This module provides types for arf-console history management.
//! [`HistoryExtraInfo`] implements reedline's `HistoryItemExtraInfo` trait and
//! can be stored in the `more_info` field of `HistoryItem<HistoryExtraInfo>`.
//!
//! Note: reedline's `History` trait currently uses `HistoryItem<IgnoreAllExtraInfo>`
//! (non-generic), so storing custom `more_info` through the trait API is not yet
//! possible. The trait implementation is prepared here for when the `History` trait
//! becomes generic over `ExtraInfo`.
//!
//! TODO: Once reedline's `History` trait supports generic `ExtraInfo`, update
//! `FuzzyHistory` to save/load `HistoryItem<HistoryExtraInfo>` and populate the
//! `more_info` field on save.

use reedline::HistoryItemExtraInfo;
use serde::{Deserialize, Serialize};

/// Extra metadata stored alongside a history entry in the `more_info` column.
///
/// Implements [`HistoryItemExtraInfo`] so it can be used as the `more_info`
/// field of a `HistoryItem<HistoryExtraInfo>`.
///
/// New fields can be added over time without breaking existing history entries,
/// thanks to `#[serde(default)]` which fills missing fields with their defaults.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryExtraInfo {
    /// Whether this entry is a meta command (e.g. `:cd`, `:help`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub meta_command: bool,

    /// Reprex output captured for this command, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reprex_output: Option<String>,
}

impl HistoryItemExtraInfo for HistoryExtraInfo {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_empty_json() {
        let info = HistoryExtraInfo::default();
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_meta_command_serialization() {
        let info = HistoryExtraInfo {
            meta_command: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(json, r#"{"meta_command":true}"#);

        let deserialized: HistoryExtraInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, info);
    }

    #[test]
    fn test_reprex_output_serialization() {
        let info = HistoryExtraInfo {
            reprex_output: Some("```r\n1 + 1\n#> [1] 2\n```".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("reprex_output"));

        let deserialized: HistoryExtraInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, info);
    }

    #[test]
    fn test_backward_compatible_deserialization() {
        // Old entries with only meta_command should still deserialize
        // when new fields are added
        let old_json = r#"{"meta_command":true}"#;
        let info: HistoryExtraInfo = serde_json::from_str(old_json).unwrap();
        assert!(info.meta_command);
        assert_eq!(info.reprex_output, None);
    }

    #[test]
    fn test_empty_json_deserializes_to_default() {
        let info: HistoryExtraInfo = serde_json::from_str("{}").unwrap();
        assert_eq!(info, HistoryExtraInfo::default());
    }

    #[test]
    fn test_unknown_fields_are_ignored() {
        // Future fields in DB should not break older code
        let future_json = r#"{"meta_command":true,"some_future_field":"value"}"#;
        let info: HistoryExtraInfo = serde_json::from_str(future_json).unwrap();
        assert!(info.meta_command);
    }

    #[test]
    fn test_as_history_item_extra_info() {
        use reedline::HistoryItem;

        let item: HistoryItem<HistoryExtraInfo> = HistoryItem {
            id: None,
            start_timestamp: None,
            command_line: ":cd /tmp".to_string(),
            session_id: None,
            hostname: None,
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: Some(HistoryExtraInfo {
                meta_command: true,
                ..Default::default()
            }),
        };

        let json = serde_json::to_string(&item).unwrap();
        let deserialized: HistoryItem<HistoryExtraInfo> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.more_info.as_ref().unwrap().meta_command, true);
        assert_eq!(deserialized.command_line, ":cd /tmp");
    }
}
