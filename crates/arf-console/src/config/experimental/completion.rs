//! Experimental completion settings for R code and shell mode.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for shell mode completion.
///
/// Controls what is suggested when Tab is pressed in shell mode.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ShellCompletionConfig {
    /// Enable completion of executable command names from PATH.
    ///
    /// When enabled, pressing Tab at the start of a shell command suggests
    /// executable names found in PATH directories in addition to path completions.
    /// When disabled (default), only file/directory path completion is provided.
    pub command_names: bool,
}

/// Configuration for R code completion (namespace and library fuzzy matching).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RCompletionConfig {
    /// Enable fuzzy matching for R code completions.
    ///
    /// When enabled, typing `pkg::partial` uses fuzzy matching against all
    /// exported names from the package, so `sf::geo` can match `sf::st_geometry`.
    /// Also enables fuzzy matching for `library()`/`require()` package names.
    /// When disabled (default), only R's built-in prefix matching is used.
    pub fuzzy: bool,

    /// Function names that trigger package-name completion.
    ///
    /// When fuzzy matching is enabled, typing inside a call to one of these
    /// functions will complete against installed package names.
    /// Default: `["library", "require"]`.
    /// Users can add custom functions like `"box::use"`.
    #[serde(default = "default_package_functions")]
    pub package_functions: Vec<String>,

    /// Static completion settings, configured separately for each feature.
    #[serde(default)]
    pub r#static: StaticCompletionConfig,
}

/// Configuration for static R completion features.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StaticCompletionConfig {
    /// Static inspection mode for qualified-call formal completion.
    pub formals: StaticFormalsConfig,
}

/// Configuration for installed-package static formal completion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StaticFormalsConfig {
    /// Whether static formals are disabled or preferred over R completion.
    pub mode: StaticFormalsMode,
    /// Packages and exported functions that must use the R completion oracle.
    pub exclusions: StaticFormalsExclusions,
}

/// Static formal completion modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StaticFormalsMode {
    /// Use the existing R completion oracle only.
    #[default]
    Off,
    /// Prefer installed-package static formals and fall back to R.
    PreferStatic,
}

/// Exclusions from static formal completion.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StaticFormalsExclusions {
    /// Package names to exclude (case-sensitive exact matches).
    pub packages: Vec<String>,
    /// Public qualified function names such as `pkg::function` to exclude.
    pub functions: Vec<String>,
}

impl Default for StaticFormalsConfig {
    fn default() -> Self {
        Self {
            mode: StaticFormalsMode::Off,
            exclusions: StaticFormalsExclusions::default(),
        }
    }
}

fn default_package_functions() -> Vec<String> {
    vec!["library".to_string(), "require".to_string()]
}

impl Default for RCompletionConfig {
    fn default() -> Self {
        Self {
            fuzzy: false,
            package_functions: default_package_functions(),
            r#static: StaticCompletionConfig::default(),
        }
    }
}
