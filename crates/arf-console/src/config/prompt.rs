//! Prompt configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Prompt configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PromptConfig {
    /// Main prompt format.
    pub format: String,
    /// Continuation prompt for multiline input.
    pub continuation: String,
    /// Shell mode prompt format.
    /// Supports `{shell}` placeholder for shell name (e.g., "bash", "zsh").
    pub shell_format: String,
    /// Mode indicator position: "prefix", "suffix", or "none".
    pub mode_indicator: ModeIndicatorPosition,
    /// Custom text for mode indicators.
    pub indicators: Indicators,
    /// Command status indicator configuration.
    pub status: StatusConfig,
    /// Vi mode indicator configuration.
    pub vi: ViConfig,
}

impl Default for PromptConfig {
    fn default() -> Self {
        PromptConfig {
            format: "{status}R {version}> ".to_string(),
            continuation: "+  ".to_string(),
            shell_format: "[{shell}] $ ".to_string(),
            mode_indicator: ModeIndicatorPosition::default(),
            indicators: Indicators::default(),
            status: StatusConfig::default(),
            vi: ViConfig::default(),
        }
    }
}

/// Symbols displayed in the prompt to indicate command success or failure.
///
/// Example: `symbol = { error = "✗ " }` or `symbol = { success = "✓ ", error = "✗ " }`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StatusSymbol {
    /// Symbol shown on success (default: "" - no symbol).
    #[serde(default)]
    pub success: String,
    /// Symbol shown on error (default: "✗ ").
    #[serde(default = "default_error_symbol")]
    pub error: String,
}

fn default_error_symbol() -> String {
    "✗ ".to_string()
}

impl Default for StatusSymbol {
    fn default() -> Self {
        Self {
            success: String::new(),
            error: default_error_symbol(),
        }
    }
}

/// Command status indicator configuration.
///
/// Controls how the prompt indicates success or failure of the previous command.
/// Use `symbol` to configure what symbols are displayed via the `{status}` placeholder.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct StatusConfig {
    /// Symbols to display for success/error status.
    /// Example: `symbol = { error = "✗ " }`
    #[serde(default)]
    pub symbol: StatusSymbol,
    /// Also change entire prompt color based on status.
    /// When true, the prompt color is overridden with status colors.
    pub override_prompt_color: bool,
}

/// Symbols displayed in the prompt to indicate vi editing mode.
///
/// Example: `symbol = { insert = "[I] ", normal = "[N] " }`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub(crate) struct ViSymbol {
    /// Symbol shown in vi insert mode (default: "[I] ").
    #[serde(default = "default_vi_insert_symbol")]
    pub insert: String,
    /// Symbol shown in vi normal mode (default: "[N] ").
    #[serde(default = "default_vi_normal_symbol")]
    pub normal: String,
    /// Symbol shown in non-vi modes like Emacs (default: "").
    /// Use this to maintain consistent prompt appearance across all editor modes.
    #[serde(default)]
    pub non_vi: String,
}

fn default_vi_insert_symbol() -> String {
    "[I] ".to_string()
}

fn default_vi_normal_symbol() -> String {
    "[N] ".to_string()
}

impl Default for ViSymbol {
    fn default() -> Self {
        Self {
            insert: default_vi_insert_symbol(),
            normal: default_vi_normal_symbol(),
            non_vi: String::new(),
        }
    }
}

/// Vi mode indicator configuration.
///
/// Controls symbols shown via `render_prompt_indicator()` for different editing modes.
/// Symbols appear at the end of the prompt line (after the main prompt text).
/// This is the same approach used by nushell, due to reedline's fixed render order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct ViConfig {
    /// Symbols to display for insert/normal mode.
    /// Example: `symbol = { insert = "[I] ", normal = "[N] " }`
    #[serde(default)]
    pub symbol: ViSymbol,
}

/// Position of the mode indicator relative to the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ModeIndicatorPosition {
    /// Show mode indicator before the prompt (e.g., "[reprex] r> ").
    #[default]
    Prefix,
    /// Show mode indicator after the prompt (e.g., "r> [reprex]").
    Suffix,
    /// Do not show mode indicator.
    None,
}

/// Text strings for mode indicators shown in the prompt.
///
/// These are the prefix/suffix texts that indicate special modes like reprex.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Indicators {
    /// Indicator text for reprex mode (default: "[reprex] ").
    pub reprex: String,
    /// Indicator text for auto-format mode (default: "[format] ").
    /// Shown when both reprex mode and auto-format are enabled.
    pub autoformat: String,
}

impl Default for Indicators {
    fn default() -> Self {
        Self {
            reprex: "[reprex] ".to_string(),
            autoformat: "[format] ".to_string(),
        }
    }
}
