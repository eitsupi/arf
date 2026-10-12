//! Experimental features configuration.

mod completion;
mod prompt;
mod r_source;
#[cfg(test)]
mod tests;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub use completion::StaticFormalsMode;
use completion::{RCompletionConfig, ShellCompletionConfig};
use prompt::SpinnerConfigSchema;
pub use prompt::{PromptDurationConfig, SpinnerConfig};
pub use r_source::RSourceOverride;

/// Experimental features configuration.
///
/// Features in this section are subject to change or removal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ExperimentalConfig {
    /// Sponge-like automatic removal of failed commands from history.
    ///
    /// Similar to fish's sponge plugin, this feature removes commands that
    /// produced errors from the history after a configurable delay.
    #[serde(default)]
    pub history_forget: HistoryForgetConfig,

    /// Minimum characters to trigger automatic completion display.
    ///
    /// When set, the completion menu appears automatically after typing
    /// this many characters, without requiring a Tab key press.
    /// This is similar to radian's `complete_while_typing` feature.
    ///
    /// When not set (null/omitted), completion requires Tab key press
    /// (the default behavior).
    #[serde(default)]
    pub completion_min_chars: Option<usize>,

    /// Spinner configuration for busy indicator during R execution.
    #[serde(default)]
    pub prompt_spinner: SpinnerConfig,

    /// Command duration configuration for the `{duration}` prompt placeholder.
    #[serde(default)]
    pub prompt_duration: PromptDurationConfig,

    /// R code completion configuration (fuzzy matching, package functions).
    #[serde(default)]
    pub r_completion: RCompletionConfig,

    /// Experimental R help viewer configuration (native pager opt-in).
    #[serde(default)]
    pub r_help: RHelpConfig,

    /// Shell mode completion configuration.
    #[serde(default)]
    pub shell_completion: ShellCompletionConfig,

    /// Enable `;` as a shortcut to switch to shell mode.
    ///
    /// When enabled, pressing `;` at an empty R prompt instantly switches to
    /// shell mode without requiring Enter — similar to Julia REPL behavior.
    /// When the buffer is not empty, `;` inserts a semicolon as usual.
    ///
    /// Disabled by default.
    #[serde(default)]
    pub shell_semicolon_shortcut: bool,

    /// Fish-style abbreviations for shell mode.
    ///
    /// Abbreviations are expanded automatically when you press Space or Enter.
    /// Only applies to the shell editor (not the R editor).
    ///
    /// Format: `"abbreviation" = "expansion"`
    /// Example: `"gc" = "git commit"`
    #[serde(default)]
    pub shell_abbreviations: BTreeMap<String, String>,

    /// Ordered providers for experimental R source resolution.
    #[serde(default)]
    pub r_source_overrides: Vec<RSourceOverride>,
}

/// Help viewer behavior for the interactive R REPL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum HelpViewer {
    /// Use arf's native help pager when safe to install, otherwise R's viewer.
    Auto,
    /// Leave help output entirely to R (default).
    #[default]
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

/// Experimental R help configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct RHelpConfig {
    /// Viewer to use for help requests in interactive sessions.
    pub viewer: HelpViewer,
}

/// Schema-only mirror of `ExperimentalConfig` that can derive `JsonSchema`.
///
/// This allows `schemars` to auto-generate detailed metadata (defaults,
/// numeric formats, minimum values, and full documentation), which would
/// otherwise be lost in a fully manual `JsonSchema` implementation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
struct ExperimentalConfigSchema {
    /// Sponge-like automatic removal of failed commands from history.
    ///
    /// Similar to fish's sponge plugin, this feature removes commands that
    /// produced errors from the history after a configurable delay.
    pub history_forget: HistoryForgetConfig,

    /// Minimum characters to trigger automatic completion display.
    ///
    /// When set, the completion menu appears automatically after typing
    /// this many characters, without requiring a Tab key press.
    /// This is similar to radian's `complete_while_typing` feature.
    ///
    /// When not set (null/omitted), completion requires Tab key press
    /// (the default behavior).
    pub completion_min_chars: Option<usize>,

    /// Spinner configuration for busy indicator during R execution.
    pub prompt_spinner: SpinnerConfigSchema,

    /// Command duration configuration for the `{duration}` prompt placeholder.
    pub prompt_duration: PromptDurationConfig,

    /// R code completion configuration (fuzzy matching, package functions).
    pub r_completion: RCompletionConfig,

    /// Experimental R help viewer configuration (native pager opt-in).
    pub r_help: RHelpConfig,

    /// Shell mode completion configuration.
    pub shell_completion: ShellCompletionConfig,

    /// Enable `;` as a shortcut to switch to shell mode.
    ///
    /// When enabled, pressing `;` at an empty R prompt instantly switches to
    /// shell mode without requiring Enter — similar to Julia REPL behavior.
    /// When the buffer is not empty, `;` inserts a semicolon as usual.
    ///
    /// Disabled by default.
    pub shell_semicolon_shortcut: bool,

    /// Fish-style abbreviations for shell mode.
    ///
    /// Abbreviations are expanded automatically when you press Space or Enter.
    /// Only applies to the shell editor (not the R editor).
    ///
    /// Format: `"abbreviation" = "expansion"`
    /// Example: `"gc" = "git commit"`
    pub shell_abbreviations: BTreeMap<String, String>,

    /// Ordered providers for experimental R source resolution.
    pub r_source_overrides: Vec<RSourceOverride>,
}

// Manual JsonSchema implementation for ExperimentalConfig since nu_ansi_term::Color
// doesn't implement JsonSchema. We delegate to a schema-only mirror type so that
// schemars can still auto-generate rich metadata (defaults, formats, etc.).
impl JsonSchema for ExperimentalConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ExperimentalConfig")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<ExperimentalConfigSchema>()
    }
}

/// Configuration for automatic removal of failed commands from history.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HistoryForgetConfig {
    /// Enable automatic removal of failed commands.
    pub enabled: bool,

    /// Number of failed commands to keep before purging older ones.
    /// For example, with delay = 2, the last 2 failed commands are kept
    /// accessible for quick retry, while older ones are deleted.
    pub delay: usize,

    /// If true, only purge failed commands when the session ends.
    /// If false, purge on each prompt redraw.
    pub on_exit_only: bool,
}

impl Default for HistoryForgetConfig {
    fn default() -> Self {
        HistoryForgetConfig {
            enabled: false,
            delay: 2,
            on_exit_only: false,
        }
    }
}
