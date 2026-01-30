//! Experimental features configuration.

use nu_ansi_term::Color;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Experimental features configuration.
///
/// Features in this section are subject to change or removal.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Spinner configuration for busy indicator during R execution.
    #[serde(default)]
    pub prompt_spinner: SpinnerConfig,
}

// Manual JsonSchema implementation for ExperimentalConfig since nu_ansi_term::Color
// doesn't implement JsonSchema.
impl JsonSchema for ExperimentalConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ExperimentalConfig")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "description": "Experimental features configuration. Features in this section are subject to change or removal.",
            "properties": {
                "history_forget": generator.subschema_for::<HistoryForgetConfig>(),
                "completion_min_chars": {
                    "type": ["integer", "null"],
                    "description": "Minimum characters to trigger automatic completion display"
                },
                "prompt_spinner": {
                    "type": "object",
                    "description": "Spinner configuration for busy indicator during R execution",
                    "properties": {
                        "frames": {
                            "type": "string",
                            "description": "Animation frames (each character is one frame). Empty string to disable.",
                            "default": "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
                        },
                        "color": {
                            "type": "string",
                            "description": "Color for the spinner (e.g., 'Cyan', 'LightBlue')",
                            "default": "Cyan"
                        }
                    }
                }
            }
        })
    }
}

impl Default for ExperimentalConfig {
    fn default() -> Self {
        ExperimentalConfig {
            history_forget: HistoryForgetConfig::default(),
            completion_min_chars: None,
            prompt_spinner: SpinnerConfig::default(),
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
