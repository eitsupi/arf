//! Mode configuration.
//!
//! Static configuration for various modes. These settings are not changeable at runtime.
//! For initial mode state (enabled/disabled), see `startup.mode`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Mode configuration container.
///
/// Contains static configuration for modes like reprex.
/// The initial enabled state of modes is configured in `startup.mode`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct ModeConfig {
    /// Reprex mode static configuration.
    #[serde(default)]
    pub reprex: ReprexConfig,
}

/// Reprex mode static configuration.
///
/// These settings are not changeable at runtime.
/// For enabling/disabling reprex mode, see `startup.mode.reprex`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ReprexConfig {
    /// Comment prefix for output (default: "#> ").
    pub comment: String,
}

impl Default for ReprexConfig {
    fn default() -> Self {
        ReprexConfig {
            comment: "#> ".to_string(),
        }
    }
}
