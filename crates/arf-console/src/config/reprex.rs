//! Reprex configuration.
//!
//! Static settings for reprex output and its formatter backend.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Formatter backend selector used by reprex format mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReprexFormatter {
    #[default]
    Auto,
    Air,
    Arity,
}

impl fmt::Display for ReprexFormatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Air => "air",
            Self::Arity => "arity",
        })
    }
}

/// Resolved executable formatter backend.
///
/// This type deliberately has no `Auto` variant: command invocation and code
/// formatting always operate on a concrete backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatterBackend {
    Air,
    Arity,
}

impl FormatterBackend {
    /// Human-readable backend name for diagnostics.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Air => "Air",
            Self::Arity => "Arity",
        }
    }

    /// Executable used to invoke this formatter backend.
    pub const fn command(self) -> &'static str {
        match self {
            Self::Air => "air",
            Self::Arity => "arity",
        }
    }

    /// Installation URL used in diagnostics when the backend is unavailable.
    pub const fn install_url(self) -> &'static str {
        match self {
            Self::Air => "https://github.com/posit-dev/air",
            Self::Arity => "https://github.com/jolars/arity",
        }
    }

    /// Minimum backend version required by arf's stdin integration.
    pub const fn minimum_version(self) -> &'static str {
        match self {
            Self::Air => "0.9.0",
            Self::Arity => "0.18.0",
        }
    }
}

impl fmt::Display for FormatterBackend {
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
    /// Formatter selector (`auto`, `air`, or `arity`).
    pub formatter: ReprexFormatter,
}

impl Default for ReprexConfig {
    fn default() -> Self {
        ReprexConfig {
            comment: "#> ".to_string(),
            formatter: ReprexFormatter::Auto,
        }
    }
}
