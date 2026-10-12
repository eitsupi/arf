//! Experimental prompt spinner and command duration settings.

use nu_ansi_term::Color;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Schema-only version of `SpinnerConfig` that avoids depending on `nu_ansi_term::Color`.
///
/// This type is used solely for JSON Schema generation, so that we can still
/// provide a helpful schema while `Color` does not implement `JsonSchema`.
/// The `color` field uses a manual schema via `color_prop!` for proper oneOf typing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct SpinnerConfigSchema {
    /// Spinner animation frames as a string where each character is one frame.
    /// Empty string disables the spinner.
    ///
    /// Example: `frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"` (braille dots)
    /// Example: `frames = "|/-\\"` (ASCII spinner)
    pub frames: String,

    /// Color for the spinner.
    pub color: String,
}

impl Default for SpinnerConfigSchema {
    fn default() -> Self {
        SpinnerConfigSchema {
            frames: String::new(),
            color: "Cyan".to_string(),
        }
    }
}

// Manual JsonSchema for SpinnerConfigSchema to use proper color oneOf schema.
impl JsonSchema for SpinnerConfigSchema {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SpinnerConfigSchema")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        use crate::config::colors::color_prop;
        schemars::json_schema!({
            "type": "object",
            "description": "Spinner configuration for busy indicator during R execution.",
            "properties": {
                "frames": {
                    "type": "string",
                    "description": r#"Spinner animation frames as a string where each character is one frame. Empty string disables the spinner. Example: "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏" (braille dots), "|/-\\" (ASCII spinner)."#,
                    "default": ""
                },
                "color": color_prop!("Color for the spinner.", default = "Cyan")
            }
        })
    }
}

/// Command duration configuration for the `{duration}` prompt placeholder.
///
/// Controls when and how command execution time is displayed in the prompt.
/// Only displayed when the command took longer than `threshold_ms`.
///
/// The `format` field uses `{value}` as a sub-placeholder for the time string.
/// The entire format string is conditionally displayed: when the duration exceeds
/// `threshold_ms`, `{value}` is replaced with the time string (e.g., "5s");
/// when below threshold, the `{duration}` prompt placeholder becomes empty.
///
/// Time format follows starship convention: "5s", "1m30s", "2h48m30s"
/// (no spaces between units, leading zero units skipped).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PromptDurationConfig {
    /// Format string for the duration display.
    ///
    /// Use `{value}` as a sub-placeholder for the time string (e.g., "5s").
    /// If `{value}` is omitted, only the static text in the format is shown.
    ///
    /// Examples:
    /// - `"{value} "` (default) — "5s " after a 5-second command
    /// - `"took {value} "` — "took 5s "
    /// - `"({value}) "` — "(5s) "
    #[serde(default = "default_duration_format")]
    pub format: String,

    /// Minimum duration in milliseconds before showing duration (default: 2000).
    /// Commands faster than this threshold will not show duration.
    #[serde(default = "default_duration_threshold_ms")]
    pub threshold_ms: u64,
}

fn default_duration_format() -> String {
    "{value} ".to_string()
}

fn default_duration_threshold_ms() -> u64 {
    2000
}

impl Default for PromptDurationConfig {
    fn default() -> Self {
        Self {
            format: default_duration_format(),
            threshold_ms: default_duration_threshold_ms(),
        }
    }
}

/// Spinner configuration for showing activity during R code execution.
///
/// The spinner is displayed at the start of the line while R is evaluating code,
/// providing visual feedback that the system is busy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpinnerConfig {
    /// Spinner animation frames as a string where each character is one frame.
    /// Empty string disables the spinner.
    ///
    /// Example: `frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"` (braille dots)
    /// Example: `frames = "|/-\\"` (ASCII spinner)
    #[serde(default = "default_spinner_frames")]
    pub frames: String,

    /// Color for the spinner.
    #[serde(default = "default_spinner_color")]
    pub color: Color,
}

fn default_spinner_frames() -> String {
    String::new() // Disabled by default (experimental feature)
}

fn default_spinner_color() -> Color {
    Color::Cyan
}

impl Default for SpinnerConfig {
    fn default() -> Self {
        Self {
            frames: default_spinner_frames(),
            color: default_spinner_color(),
        }
    }
}
