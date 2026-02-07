//! Experimental features configuration.

use nu_ansi_term::Color;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
}

/// Schema-only version of `SpinnerConfig` that avoids depending on `nu_ansi_term::Color`.
///
/// This type is used solely for JSON Schema generation, so that we can still
/// provide a helpful schema while `Color` does not implement `JsonSchema`.
/// The `color` field uses a manual schema via `color_prop!` for proper oneOf typing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SpinnerConfigSchema {
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
        use super::colors::color_prop;
        schemars::json_schema!({
            "type": "object",
            "description": "Spinner configuration for busy indicator during R execution.",
            "properties": {
                "frames": {
                    "type": "string",
                    "description": "Spinner animation frames as a string where each character is one frame. Empty string disables the spinner. Example: \"⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏\" (braille dots), \"|/-\\\\\" (ASCII spinner).",
                    "default": ""
                },
                "color": color_prop!("Color for the spinner.", default = "Cyan")
            }
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinner_config_default() {
        let config = SpinnerConfig::default();
        assert!(config.frames.is_empty()); // Disabled by default
        assert_eq!(config.color, Color::Cyan);
    }

    #[test]
    fn test_spinner_config_disabled() {
        let config = SpinnerConfig {
            frames: String::new(),
            color: Color::Default,
        };
        assert!(config.frames.is_empty());
    }

    #[test]
    fn test_spinner_config_custom_frames() {
        let config = SpinnerConfig {
            frames: "|/-\\".to_string(),
            color: Color::Green,
        };
        assert_eq!(config.frames, "|/-\\");
        assert_eq!(config.color, Color::Green);
    }

    #[test]
    fn test_experimental_config_default() {
        let config = ExperimentalConfig::default();
        assert!(!config.history_forget.enabled);
        assert!(config.completion_min_chars.is_none());
        assert!(config.prompt_spinner.frames.is_empty()); // Disabled by default
    }

    #[test]
    fn test_parse_spinner_config() {
        let toml_str = r#"
[experimental.prompt_spinner]
frames = "abc"
color = "Red"
"#;
        let config: crate::config::Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.experimental.prompt_spinner.frames, "abc");
        assert_eq!(config.experimental.prompt_spinner.color, Color::Red);
    }

    #[test]
    fn test_parse_spinner_disabled() {
        let toml_str = r#"
[experimental.prompt_spinner]
frames = ""
"#;
        let config: crate::config::Config = toml::from_str(toml_str).unwrap();
        assert!(config.experimental.prompt_spinner.frames.is_empty());
    }
}
