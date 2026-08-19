//! History configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;

/// Where command history is kept for the duration of a process.
///
/// `volatile` deliberately keeps history in an arf-owned in-memory SQLite
/// store. It never reads or writes the configured history directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryMode {
    Persistent { dir: Option<PathBuf> },
    Volatile,
}

impl HistoryMode {
    /// Return the configured directory when persistence is selected.
    pub fn persistent_dir(&self) -> Option<&PathBuf> {
        match self {
            Self::Persistent { dir } => dir.as_ref(),
            Self::Volatile => None,
        }
    }
}

/// History configuration.
#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// Maximum height (rows) for the history search menu (Ctrl+R).
    /// The actual height is the minimum of this value and the terminal height minus overhead.
    pub menu_max_height: u16,

    /// Persistent or session-only history behavior.
    pub mode: HistoryMode,
}

impl Serialize for HistoryConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum WireMode<'a> {
            Name(&'a str),
            Persistent { dir: &'a PathBuf },
        }

        #[derive(Serialize)]
        struct WireHistory<'a> {
            menu_max_height: u16,
            mode: WireMode<'a>,
        }

        let mode = match &self.mode {
            HistoryMode::Persistent { dir: Some(dir) } => WireMode::Persistent { dir },
            HistoryMode::Persistent { dir: None } => WireMode::Name("persistent"),
            HistoryMode::Volatile => WireMode::Name("volatile"),
        };
        WireHistory {
            menu_max_height: self.menu_max_height,
            mode,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HistoryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawMode {
            Name(String),
            Persistent(RawPersistentMode),
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawPersistentMode {
            dir: PathBuf,
        }

        #[derive(Deserialize)]
        struct RawHistoryConfig {
            #[serde(default = "default_menu_max_height")]
            menu_max_height: u16,
            #[serde(default)]
            mode: Option<RawMode>,
            #[serde(default)]
            dir: Option<PathBuf>,
            #[serde(default)]
            disabled: Option<bool>,
        }

        let raw = RawHistoryConfig::deserialize(deserializer)?;
        let mode = match raw.mode {
            Some(RawMode::Name(mode)) => {
                if raw.dir.is_some() {
                    return Err(serde::de::Error::custom(
                        "history.dir cannot be used when history.mode is set",
                    ));
                }
                match (mode.as_str(), raw.disabled) {
                    ("persistent", _) => HistoryMode::Persistent { dir: None },
                    ("volatile", _) => HistoryMode::Volatile,
                    _ => {
                        return Err(serde::de::Error::unknown_variant(
                            &mode,
                            &["persistent", "volatile"],
                        ));
                    }
                }
            }
            Some(RawMode::Persistent(RawPersistentMode { dir })) => {
                if raw.dir.is_some() {
                    return Err(serde::de::Error::custom(
                        "history.dir cannot be used when history.mode is set",
                    ));
                }
                HistoryMode::Persistent { dir: Some(dir) }
            }
            None => match (raw.disabled, raw.dir) {
                (Some(true), _) => HistoryMode::Volatile,
                (Some(false), dir) | (None, dir) => HistoryMode::Persistent { dir },
            },
        };
        Ok(Self {
            menu_max_height: raw.menu_max_height,
            mode,
        })
    }
}

impl JsonSchema for HistoryConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("HistoryConfig")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "History configuration.",
            "type": "object",
            "properties": {
                "menu_max_height": {
                    "description": "Maximum height (rows) for the history search menu (Ctrl+R).",
                    "type": "integer",
                    "format": "uint16",
                    "default": 15,
                    "maximum": 65535,
                    "minimum": 0
                },
                "mode": {
                    "default": "persistent",
                    "oneOf": [
                        {
                            "type": "string",
                            "enum": ["persistent", "volatile"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "dir": { "type": "string" }
                            },
                            "required": ["dir"],
                            "additionalProperties": false
                        }
                    ]
                }
            }
        })
    }
}

fn default_menu_max_height() -> u16 {
    15
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            menu_max_height: 15,
            mode: HistoryMode::Persistent { dir: None },
        }
    }
}
