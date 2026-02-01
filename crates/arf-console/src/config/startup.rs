//! Startup configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Startup configuration.
///
/// Settings in this section are evaluated once at startup and do not change during the session.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StartupConfig {
    /// How to locate R installation.
    #[serde(default)]
    pub r_source: RSource,
    /// Show startup banner.
    #[serde(default = "default_true")]
    pub show_banner: bool,
    /// Initial mode settings (reprex, autoformat).
    #[serde(default)]
    pub mode: StartupModeConfig,
}

fn default_true() -> bool {
    true
}

impl Default for StartupConfig {
    fn default() -> Self {
        StartupConfig {
            r_source: RSource::default(),
            show_banner: true,
            mode: StartupModeConfig::default(),
        }
    }
}

/// Initial mode configuration.
///
/// These modes can be toggled at runtime via meta commands, but
/// this configuration determines their initial state at startup.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct StartupModeConfig {
    /// Enable reprex mode (no prompt, output prefixed with comment).
    pub reprex: bool,
    /// Enable auto-formatting of R code in reprex mode (requires Air CLI).
    pub autoformat: bool,
}

/// How to locate the R installation.
///
/// Can be specified as:
/// - `"auto"` (default): Use rig if available, otherwise find R from PATH
/// - `"rig"`: Use rig's default R version (error if rig unavailable)
/// - `{ path = "/path/to/R" }`: Use explicit R_HOME path
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RSource {
    /// Use a predefined mode (auto or rig).
    Mode(RSourceMode),
    /// Use an explicit R_HOME path.
    Path {
        /// Path to R installation (R_HOME).
        path: PathBuf,
    },
}

impl Default for RSource {
    fn default() -> Self {
        RSource::Mode(RSourceMode::Auto)
    }
}

/// Predefined modes for locating R.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RSourceMode {
    /// Try rig if available, otherwise find R from PATH.
    Auto,
    /// Use rig's default R version (error if rig unavailable).
    Rig,
}

/// Describes how R was resolved at startup.
///
/// This is used to display session info and determine if features like `:switch` are available.
#[derive(Debug, Clone, Default)]
pub enum RSourceStatus {
    /// R was resolved via rig.
    Rig {
        /// The R version being used.
        version: String,
    },
    /// R was found from PATH (auto mode, rig not available).
    #[default]
    Path,
    /// R was specified via explicit path in config.
    ExplicitPath {
        /// The R_HOME path.
        path: PathBuf,
    },
}

impl RSourceStatus {
    /// Returns true if rig was used to resolve R.
    ///
    /// This determines if features like `:switch` are available.
    pub fn rig_enabled(&self) -> bool {
        matches!(self, RSourceStatus::Rig { .. })
    }

    /// Returns a human-readable description for display.
    pub fn display(&self) -> String {
        match self {
            RSourceStatus::Rig { version } => format!("rig (R {})", version),
            RSourceStatus::Path => "PATH".to_string(),
            RSourceStatus::ExplicitPath { path } => format!("path ({})", path.display()),
        }
    }
}
