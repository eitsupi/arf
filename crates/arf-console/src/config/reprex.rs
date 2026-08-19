//! Reprex configuration.
//!
//! Static settings for reprex output and its formatter backend.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Formatter backend used by reprex format mode.
///
/// Air is the only supported backend for now; additional backends are
/// intentionally deferred until their integration is designed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReprexFormatter {
    #[default]
    Air,
}

impl ReprexFormatter {
    /// Human-readable backend name for diagnostics.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Air => "Air",
        }
    }

    /// Executable used to invoke this formatter backend.
    pub const fn command(self) -> &'static str {
        match self {
            Self::Air => "air",
        }
    }

    /// Installation URL used in diagnostics when the backend is unavailable.
    pub const fn install_url(self) -> &'static str {
        match self {
            Self::Air => "https://github.com/posit-dev/air",
        }
    }
}

impl fmt::Display for ReprexFormatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.command())
    }
}

/// Reprex mode static configuration.
///
/// These settings are not changeable at runtime.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ReprexConfig {
    /// Comment prefix for output (default: "#> ").
    pub comment: String,
    /// Formatter backend (currently only `air` is supported).
    pub formatter: ReprexFormatter,
}

impl Default for ReprexConfig {
    fn default() -> Self {
        ReprexConfig {
            comment: "#> ".to_string(),
            formatter: ReprexFormatter::Air,
        }
    }
}
