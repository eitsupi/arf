//! Experimental features configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Experimental features configuration.
///
/// Features in this section are subject to change or removal.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
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
}

impl Default for ExperimentalConfig {
    fn default() -> Self {
        ExperimentalConfig {
            history_forget: HistoryForgetConfig::default(),
            completion_min_chars: None,
        }
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
