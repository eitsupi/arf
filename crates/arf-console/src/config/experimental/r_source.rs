//! Experimental R source override providers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A provider for resolving an R installation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "provider", rename_all = "kebab-case")]
pub enum RSourceOverride {
    /// Resolve R using the active pixi environment.
    Pixi,
    /// Resolve R using the synchronized rig project in the current directory.
    RigProject,
    /// Read the R version from a version file.
    VersionFile { file: PathBuf },
    /// Read the R version from a TOML key.
    TomlKey { file: PathBuf, key: String },
    /// Read the R version from a JSON key.
    JsonKey { file: PathBuf, key: String },
}
