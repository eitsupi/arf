//! R runtime configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

fn default_true() -> bool {
    true
}

/// R runtime configuration.
///
/// Controls R-specific behavior such as automatic option synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RConfig {
    /// Automatically sync R's `options(width)` with the terminal width.
    ///
    /// When enabled (default), R's `options(width)` is set to match the terminal
    /// columns at startup and updated dynamically on resize. This ensures output
    /// from functions like `str()`, `print()`, and tibble printing uses the full
    /// available terminal width instead of R's default of 80.
    #[serde(default = "default_true")]
    pub auto_width: bool,
    /// How `help()` results are displayed in the interactive REPL.
    pub help: RHelpConfig,
}

impl Default for RConfig {
    fn default() -> Self {
        RConfig {
            auto_width: true,
            help: RHelpConfig::default(),
        }
    }
}

/// Help viewer behavior for the interactive R REPL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum HelpViewer {
    /// Use arf's native help pager when safe to install, otherwise R's viewer.
    #[default]
    Auto,
    /// Leave help output entirely to R.
    R,
}

impl fmt::Display for HelpViewer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::R => "r",
        })
    }
}

/// R help configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct RHelpConfig {
    /// Viewer to use for help requests in interactive sessions.
    pub viewer: HelpViewer,
}
