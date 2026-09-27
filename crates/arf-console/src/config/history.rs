//! History configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

/// Where command history is kept for the duration of a process.
///
/// `volatile` deliberately keeps history in an arf-owned in-memory SQLite
/// store. It never reads or writes the configured history directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryMode {
    Persistent { dir: Option<PathBuf> },
    Volatile,
}

/// Origin of the effective on-disk history directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryLocationSource {
    Default,
    Explicit,
    Volatile,
}

/// One resolved history location shared by all history consumers.
///
/// Volatile mode intentionally has no directory. The resolver itself is pure:
/// callers provide the already-derived default directory, keeping environment
/// lookup outside the decision and allowing hermetic tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedHistoryLocation {
    directory: Option<PathBuf>,
    source: HistoryLocationSource,
}

impl ResolvedHistoryLocation {
    pub(crate) fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    pub(crate) fn source(&self) -> HistoryLocationSource {
        self.source
    }

    pub(crate) fn database_path(&self, filename: &str) -> Option<PathBuf> {
        self.directory
            .as_ref()
            .map(|directory| directory.join(filename))
    }
}

pub(crate) fn resolve_history_location(
    mode: &HistoryMode,
    default_directory: Option<PathBuf>,
) -> ResolvedHistoryLocation {
    match mode {
        HistoryMode::Persistent {
            dir: Some(directory),
        } => ResolvedHistoryLocation {
            directory: Some(directory.clone()),
            source: HistoryLocationSource::Explicit,
        },
        HistoryMode::Persistent { dir: None } => ResolvedHistoryLocation {
            directory: default_directory,
            source: HistoryLocationSource::Default,
        },
        HistoryMode::Volatile => ResolvedHistoryLocation {
            directory: None,
            source: HistoryLocationSource::Volatile,
        },
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

#[cfg(test)]
mod location_tests {
    use super::*;

    #[test]
    fn resolver_preserves_source_and_never_assigns_volatile_directory() {
        let default = PathBuf::from("/xdg/arf/history");
        let resolved = resolve_history_location(
            &HistoryMode::Persistent { dir: None },
            Some(default.clone()),
        );
        assert_eq!(resolved.directory(), Some(default.as_path()));
        assert_eq!(resolved.source(), HistoryLocationSource::Default);
        assert_eq!(resolved.database_path("r.db"), Some(default.join("r.db")));

        let explicit = PathBuf::from("/custom/history");
        let resolved = resolve_history_location(
            &HistoryMode::Persistent {
                dir: Some(explicit.clone()),
            },
            Some(default),
        );
        assert_eq!(resolved.directory(), Some(explicit.as_path()));
        assert_eq!(resolved.source(), HistoryLocationSource::Explicit);

        let resolved = resolve_history_location(&HistoryMode::Volatile, None);
        assert_eq!(resolved.directory(), None);
        assert_eq!(resolved.database_path("r.db"), None);
        assert_eq!(resolved.source(), HistoryLocationSource::Volatile);
    }
}
