//! Startup configuration.

use clap::ValueEnum;
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
    /// Initial reprex mode.
    #[serde(default)]
    pub reprex: ReprexMode,
}

fn default_true() -> bool {
    true
}

impl Default for StartupConfig {
    fn default() -> Self {
        StartupConfig {
            r_source: RSource::default(),
            show_banner: true,
            reprex: ReprexMode::Off,
        }
    }
}

/// Reprex execution mode.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ValueEnum, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum ReprexMode {
    /// Evaluate normally.
    #[default]
    Off,
    /// Evaluate and show reprex output.
    On,
    /// Format code with the configured formatter before reprex evaluation.
    Format,
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

/// Metadata describing an R source override that selected the active R version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RSourceOverrideInfo {
    /// The provider that supplied the version.
    pub provider: String,
    /// The file read by the provider.
    pub file: PathBuf,
    /// The key read by the provider, if any.
    pub key: Option<String>,
    /// The version specification read from the provider.
    pub requested_version: String,
    /// The installed version selected by the specification.
    pub resolved_version: String,
}

impl RSourceOverrideInfo {
    /// Format the override metadata for startup and session displays.
    pub fn display(&self) -> String {
        let source = match &self.key {
            Some(key) => format!("{}:{}", self.file.display(), key),
            None => self.file.display().to_string(),
        };
        format!(
            r#"{} {} = "{}""#,
            self.provider, source, self.requested_version
        )
    }
}

/// Describes how R was resolved at startup.
///
/// This is used to display session info and determine if features like `:switch` are available.
#[derive(Debug, Clone, Default)]
pub enum RSourceStatus {
    /// R belongs to an externally activated Pixi environment.
    Pixi { info: PixiRSourceInfo },
    /// R belongs to a synchronized rig project.
    RigProject { info: RigProjectRSourceInfo },
    /// R was resolved via rig.
    Rig {
        /// The R version being used.
        version: String,
        /// Metadata for the override that selected this version, if any.
        override_info: Option<RSourceOverrideInfo>,
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
        matches!(
            self,
            RSourceStatus::Rig { .. } | RSourceStatus::RigProject { .. }
        )
    }

    /// Return the R installation verified by an environment provider, if any.
    pub fn verified_target(&self) -> Option<&VerifiedRTarget> {
        match self {
            Self::Pixi { info } => Some(&info.target),
            Self::RigProject { info } => Some(&info.target),
            Self::Rig { .. } | Self::Path | Self::ExplicitPath { .. } => None,
        }
    }

    /// Returns a human-readable description for display.
    pub fn display(&self) -> String {
        match self {
            RSourceStatus::Rig {
                version,
                override_info,
            } => match override_info {
                Some(info) => format!("rig (R {}; override: {})", version, info.display()),
                None => format!("rig (R {})", version),
            },
            RSourceStatus::Path => "PATH".to_string(),
            RSourceStatus::ExplicitPath { path } => format!("path ({})", path.display()),
            RSourceStatus::Pixi { info } => match &info.version {
                Some(version) => format!("pixi (environment {}; R {})", info.environment, version),
                None => format!("pixi (environment {})", info.environment),
            },
            RSourceStatus::RigProject { info } => format!(
                "rig project ({}; R installation {})",
                info.root.display(),
                info.installation_name
            ),
        }
    }
}

/// R installation verified by an environment provider before it is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRTarget {
    /// The executable a client runs to start this R, such as a project wrapper.
    pub binary: PathBuf,
    pub home: PathBuf,
    /// The R shared library loaded by arf.
    pub library: PathBuf,
}

/// Identity and verified R target of an externally activated Pixi environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixiRSourceInfo {
    pub manifest: PathBuf,
    pub environment: String,
    pub prefix: PathBuf,
    pub target: VerifiedRTarget,
    pub version: Option<String>,
}

/// Identity and verified R target of a synchronized rig project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigProjectRSourceInfo {
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub target: VerifiedRTarget,
    /// rig's installation name, which is not necessarily an R release version.
    pub installation_name: String,
}
