//! History configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// History configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HistoryConfig {
    /// Maximum height (rows) for the history search menu (Ctrl+R).
    /// The actual height is the minimum of this value and the terminal height minus overhead.
    pub menu_max_height: u16,

    /// Disable history entirely.
    #[serde(default)]
    pub disabled: bool,

    /// Custom history directory (overrides default XDG location).
    /// R history will be stored at `{dir}/r.db`, Shell at `{dir}/shell.db`.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            menu_max_height: 15,
            disabled: false,
            dir: None,
        }
    }
}
