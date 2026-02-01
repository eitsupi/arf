//! Color configuration for syntax highlighting.

use nu_ansi_term::Color;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Color configuration for syntax highlighting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    /// Colors for R syntax highlighting.
    pub r: RColorConfig,
    /// Colors for meta command highlighting.
    pub meta: MetaColorConfig,
    /// Colors for prompts.
    pub prompt: PromptColorConfig,
}

// Manual JsonSchema implementation for ColorsConfig since nu_ansi_term::Color
// doesn't implement JsonSchema. We provide a descriptive schema instead.
impl JsonSchema for ColorsConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ColorsConfig")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "description": "Color configuration for syntax highlighting and prompts. Colors can be named (e.g., 'Red', 'DarkGray'), 256-color ({ Fixed: 99 }), or RGB ({ Rgb: [255, 0, 0] }).",
            "properties": {
                "r": {
                    "type": "object",
                    "description": "Colors for R syntax tokens",
                    "properties": {
                        "comment": { "description": "Color for comments" },
                        "string": { "description": "Color for string literals" },
                        "number": { "description": "Color for numeric literals" },
                        "keyword": { "description": "Color for keywords" },
                        "constant": { "description": "Color for constants (TRUE, FALSE, NULL, NA, etc.)" },
                        "operator": { "description": "Color for operators" },
                        "punctuation": { "description": "Color for punctuation" },
                        "identifier": { "description": "Color for identifiers" }
                    }
                },
                "meta": {
                    "type": "object",
                    "description": "Colors for meta commands",
                    "properties": {
                        "command": { "description": "Color for meta command lines" }
                    }
                },
                "prompt": {
                    "type": "object",
                    "description": "Colors for prompts",
                    "properties": {
                        "main": { "description": "Color for the main R prompt" },
                        "continuation": { "description": "Color for the continuation prompt" },
                        "shell": { "description": "Color for the shell mode prompt" },
                        "indicator": { "description": "Color for the mode indicator text" },
                        "status": {
                            "type": "object",
                            "description": "Colors for command status indicator",
                            "properties": {
                                "success": { "description": "Color for success status" },
                                "error": { "description": "Color for error status" }
                            }
                        },
                        "vi": {
                            "type": "object",
                            "description": "Colors for vi mode indicator",
                            "properties": {
                                "insert": { "description": "Color for vi insert mode" },
                                "normal": { "description": "Color for vi normal mode" },
                                "non_vi": { "description": "Color for non-vi modes (Emacs, etc.)" }
                            }
                        }
                    }
                }
            }
        })
    }
}

impl Default for ColorsConfig {
    fn default() -> Self {
        ColorsConfig {
            r: RColorConfig::default(),
            meta: MetaColorConfig::default(),
            prompt: PromptColorConfig::default(),
        }
    }
}

/// Color configuration for R syntax tokens.
///
/// Each field accepts a color value. Supported colors:
/// - Named: Black, Red, Green, Yellow, Blue, Purple, Magenta, Cyan, White
/// - Light: LightRed, LightGreen, LightYellow, LightBlue, LightPurple, LightMagenta, LightCyan, LightGray
/// - Dark: DarkGray
/// - Special: Default (terminal default color)
/// - 256-color: { Fixed = 0-255 }
/// - True color: { Rgb = [r, g, b] }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RColorConfig {
    /// Color for comments (lines starting with #).
    pub comment: Color,
    /// Color for string literals.
    pub string: Color,
    /// Color for numeric literals.
    pub number: Color,
    /// Color for keywords (if, else, for, while, function, etc.).
    pub keyword: Color,
    /// Color for constants (TRUE, FALSE, NULL, NA, Inf, NaN).
    pub constant: Color,
    /// Color for operators (+, -, <-, |>, etc.).
    pub operator: Color,
    /// Color for punctuation (brackets, commas, semicolons).
    pub punctuation: Color,
    /// Color for identifiers (variable and function names).
    pub identifier: Color,
}

impl Default for RColorConfig {
    fn default() -> Self {
        RColorConfig {
            comment: Color::DarkGray,
            string: Color::Green,
            number: Color::LightMagenta,
            keyword: Color::LightBlue,
            constant: Color::LightCyan,
            operator: Color::Yellow,
            punctuation: Color::Default,
            identifier: Color::Default,
        }
    }
}

/// Color configuration for meta commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetaColorConfig {
    /// Color for meta command lines (starting with :).
    pub command: Color,
}

impl Default for MetaColorConfig {
    fn default() -> Self {
        MetaColorConfig {
            command: Color::Magenta,
        }
    }
}

/// Color configuration for prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptColorConfig {
    /// Color for the main R prompt.
    pub main: Color,
    /// Color for the continuation prompt (multiline input).
    pub continuation: Color,
    /// Color for the shell mode prompt.
    pub shell: Color,
    /// Color for the mode indicator text ([reprex], [format], #!).
    pub indicator: Color,
    /// Colors for command status indicator.
    pub status: StatusColorConfig,
    /// Colors for vi mode indicator.
    pub vi: ViColorConfig,
}

impl Default for PromptColorConfig {
    fn default() -> Self {
        PromptColorConfig {
            main: Color::LightGreen,
            continuation: Color::LightGreen,
            shell: Color::LightRed,
            indicator: Color::Yellow,
            status: StatusColorConfig::default(),
            vi: ViColorConfig::default(),
        }
    }
}

/// Color configuration for command status indicator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusColorConfig {
    /// Color for success status (used when mode = "color" or "both").
    pub success: Color,
    /// Color for error status (used when mode = "color" or "both").
    pub error: Color,
}

impl Default for StatusColorConfig {
    fn default() -> Self {
        StatusColorConfig {
            success: Color::LightGreen,
            error: Color::LightRed,
        }
    }
}

/// Color configuration for vi mode indicator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViColorConfig {
    /// Color for vi insert mode indicator.
    pub insert: Color,
    /// Color for vi normal mode indicator.
    pub normal: Color,
    /// Color for non-vi modes (Emacs, etc.).
    pub non_vi: Color,
}

impl Default for ViColorConfig {
    fn default() -> Self {
        ViColorConfig {
            insert: Color::Default,
            normal: Color::Default,
            non_vi: Color::Default,
        }
    }
}
