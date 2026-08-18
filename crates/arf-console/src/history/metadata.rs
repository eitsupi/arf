//! Lossless metadata for arf-owned history rows.
//!
//! [`HistoryExtraInfo`] stores the complete JSON object found in reedline's
//! `more_info` column.  Known fields are exposed through typed accessors, but
//! unknown fields are retained so that updating a row with an older arf does
//! not erase metadata written by a newer one.  A missing `more_info` column is
//! represented by `None` on the history item; an ordinary known entry uses an
//! empty JSON object.

use reedline::HistoryItemExtraInfo;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

const META_COMMAND_KEY: &str = "meta_command";
#[allow(dead_code)]
const REPREX_OUTPUT_KEY: &str = "reprex_output";

/// The complete JSON object stored as history metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryExtraInfo {
    fields: Map<String, Value>,
}

impl HistoryExtraInfo {
    /// Return the known meta-command flag, if it has a JSON boolean value.
    #[allow(dead_code)]
    pub fn meta_command(&self) -> Option<bool> {
        self.fields.get(META_COMMAND_KEY).and_then(Value::as_bool)
    }

    /// Set the known meta-command flag.
    ///
    /// `false` removes the key so ordinary commands retain the compact `{}`
    /// representation.
    pub fn set_meta_command(&mut self, value: bool) {
        if value {
            self.fields
                .insert(META_COMMAND_KEY.to_string(), Value::Bool(true));
        } else {
            self.fields.remove(META_COMMAND_KEY);
        }
    }

    /// Return captured reprex output, if it is a JSON string.
    #[allow(dead_code)]
    pub fn reprex_output(&self) -> Option<&str> {
        self.fields.get(REPREX_OUTPUT_KEY).and_then(Value::as_str)
    }

    /// Set captured reprex output, removing the key for `None`.
    #[allow(dead_code)]
    pub fn set_reprex_output(&mut self, value: Option<String>) {
        match value {
            Some(value) => {
                self.fields
                    .insert(REPREX_OUTPUT_KEY.to_string(), Value::String(value));
            }
            None => {
                self.fields.remove(REPREX_OUTPUT_KEY);
            }
        }
    }
}

impl Serialize for HistoryExtraInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.fields.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HistoryExtraInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Object(fields) => Ok(Self { fields }),
            other => Err(serde::de::Error::custom(format!(
                "history metadata must be a JSON object, got {other}"
            ))),
        }
    }
}

impl HistoryItemExtraInfo for HistoryExtraInfo {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty_json() {
        assert_eq!(
            serde_json::to_string(&HistoryExtraInfo::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn typed_accessors_use_compact_representations() {
        let mut info = HistoryExtraInfo::default();
        assert_eq!(info.meta_command(), None);
        info.set_meta_command(true);
        assert_eq!(info.meta_command(), Some(true));
        info.set_meta_command(false);
        assert_eq!(info.meta_command(), None);

        info.set_reprex_output(Some("output".to_string()));
        assert_eq!(info.reprex_output(), Some("output"));
        info.set_reprex_output(None);
        assert_eq!(info.reprex_output(), None);
        assert_eq!(serde_json::to_string(&info).unwrap(), "{}");
    }

    #[test]
    fn unknown_fields_survive_typed_updates() {
        let mut info: HistoryExtraInfo =
            serde_json::from_str(r#"{"meta_command":true,"future":{"value":1}}"#).unwrap();
        info.set_meta_command(false);
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"future":{"value":1}}"#
        );
    }

    #[test]
    fn unexpected_known_field_types_are_preserved_until_set() {
        let mut info: HistoryExtraInfo =
            serde_json::from_str(r#"{"meta_command":"future"}"#).unwrap();
        assert_eq!(info.meta_command(), None);
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"meta_command":"future"}"#
        );
        info.set_meta_command(true);
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"meta_command":true}"#
        );
    }

    #[test]
    fn non_object_metadata_is_rejected() {
        let result = serde_json::from_str::<HistoryExtraInfo>(r#"["not an object"]"#);
        assert!(result.is_err());
    }

    #[test]
    fn history_item_round_trips_metadata() {
        let item = reedline::HistoryItem {
            id: None,
            start_timestamp: None,
            command_line: "x <- 1:10".to_string(),
            session_id: None,
            hostname: None,
            cwd: None,
            duration: None,
            exit_status: None,
            more_info: Some(HistoryExtraInfo::default()),
        };
        let json = serde_json::to_string(&item).unwrap();
        let decoded: reedline::HistoryItem<HistoryExtraInfo> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.command_line, "x <- 1:10");
        assert_eq!(decoded.more_info, Some(HistoryExtraInfo::default()));
    }
}
