//! Reprex mode configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reprex mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ReprexConfig {
    /// Enable reprex mode (no prompt, output prefixed with comment).
    pub enabled: bool,
    /// Comment prefix for output (default: "#> ").
    pub comment: String,
    /// Enable auto-formatting of R code (requires Air CLI).
    /// Previously in [formatter.enabled].
    #[serde(default)]
    pub autoformat: bool,
}

impl Default for ReprexConfig {
    fn default() -> Self {
        ReprexConfig {
            enabled: false,
            comment: "#> ".to_string(),
            autoformat: false,
        }
    }
}
