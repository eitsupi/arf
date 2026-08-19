//! History configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

/// Where command history is kept for the duration of a process.
///
/// `volatile` deliberately keeps history in an arf-owned in-memory SQLite
/// store. It never reads or writes the configured history directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum HistoryMode {
    Persistent {
        #[serde(default)]
        dir: Option<PathBuf>,
    },
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
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(default)]
pub struct HistoryConfig {
    /// Maximum height (rows) for the history search menu (Ctrl+R).
    /// The actual height is the minimum of this value and the terminal height minus overhead.
    pub menu_max_height: u16,

    /// Keep history only for the current process; do not load or save a file.
    #[serde(flatten)]
    pub mode: HistoryMode,
}

impl<'de> Deserialize<'de> for HistoryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawHistoryConfig {
            #[serde(default = "default_menu_max_height")]
            menu_max_height: u16,
            mode: Option<String>,
            #[serde(default)]
            dir: Option<PathBuf>,
            #[serde(default)]
            disabled: Option<bool>,
        }

        let raw = RawHistoryConfig::deserialize(deserializer)?;
        let mode = match (raw.mode, raw.disabled) {
            (Some(mode), _) => match mode.as_str() {
                "persistent" => HistoryMode::Persistent { dir: raw.dir },
                "volatile" => HistoryMode::Volatile,
                _ => {
                    return Err(serde::de::Error::unknown_variant(
                        &mode,
                        &["persistent", "volatile"],
                    ));
                }
            },
            (None, Some(true)) => HistoryMode::Volatile,
            (None, Some(false)) | (None, None) => HistoryMode::Persistent { dir: raw.dir },
        };
        Ok(Self {
            menu_max_height: raw.menu_max_height,
            mode,
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
