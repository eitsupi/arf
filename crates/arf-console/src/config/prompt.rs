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
    /// Spinner configuration for busy indicator during R execution.
    pub spinner: SpinnerConfig,
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
            spinner: SpinnerConfig::default(),
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
pub struct StatusConfig {
    /// Symbols to display for success/error status.
    /// Example: `symbol = { error = "✗ " }`
    #[serde(default)]
    pub symbol: StatusSymbol,
    /// Also change entire prompt color based on status.
    /// When true, the prompt color is overridden with status colors.
    pub override_prompt_color: bool,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            symbol: StatusSymbol::default(),
            override_prompt_color: false,
        }
    }
}

/// Position of the mode indicator relative to the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModeIndicatorPosition {
    /// Show mode indicator before the prompt (e.g., "[reprex] r> ").
    Prefix,
    /// Show mode indicator after the prompt (e.g., "r> [reprex]").
    Suffix,
    /// Do not show mode indicator.
    None,
}

impl Default for ModeIndicatorPosition {
    fn default() -> Self {
        Self::Prefix
    }
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

/// Spinner configuration for showing activity during R code execution.
///
/// The spinner is displayed at the start of the line while R is evaluating code,
/// providing visual feedback that the system is busy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SpinnerConfig {
    /// Spinner animation frames as a string where each character is one frame.
    /// Empty string disables the spinner.
    ///
    /// Example: `frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"` (braille dots)
    /// Example: `frames = "|/-\\"` (ASCII spinner)
    #[serde(default = "default_spinner_frames")]
    pub frames: String,
}

fn default_spinner_frames() -> String {
    "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".to_string()
}

impl Default for SpinnerConfig {
    fn default() -> Self {
        Self {
            frames: default_spinner_frames(),
        }
    }
}
