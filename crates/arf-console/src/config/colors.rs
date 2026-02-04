//! Color configuration for syntax highlighting.

use nu_ansi_term::Color;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Generate a color property schema with a description.
///
/// Colors can be:
/// - A named string: "Red", "LightBlue", "DarkGray", "Default", etc.
/// - A 256-color object: `{ Fixed: 99 }`
/// - An RGB object: `{ Rgb: [255, 0, 0] }`
macro_rules! color_prop {
    ($desc:expr) => {
        schemars::json_schema!({
            "description": $desc,
            "oneOf": [
                {
                    "type": "string",
                    "enum": [
                        "Default", "Black", "Red", "Green", "Yellow", "Blue",
                        "Purple", "Magenta", "Cyan", "White",
                        "DarkGray", "LightGray",
                        "LightRed", "LightGreen", "LightYellow", "LightBlue",
                        "LightPurple", "LightMagenta", "LightCyan"
                    ]
                },
                {
                    "type": "object",
                    "properties": {
                        "Fixed": { "type": "integer", "minimum": 0, "maximum": 255 }
                    },
                    "required": ["Fixed"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "Rgb": {
                            "type": "array",
                            "items": { "type": "integer", "minimum": 0, "maximum": 255 },
                            "minItems": 3,
                            "maxItems": 3
                        }
                    },
                    "required": ["Rgb"],
                    "additionalProperties": false
                }
            ]
        })
    };
    ($desc:expr, default = $default:expr) => {
        schemars::json_schema!({
            "description": $desc,
            "default": $default,
            "oneOf": [
                {
                    "type": "string",
                    "enum": [
                        "Default", "Black", "Red", "Green", "Yellow", "Blue",
                        "Purple", "Magenta", "Cyan", "White",
                        "DarkGray", "LightGray",
                        "LightRed", "LightGreen", "LightYellow", "LightBlue",
                        "LightPurple", "LightMagenta", "LightCyan"
                    ]
                },
                {
                    "type": "object",
                    "properties": {
                        "Fixed": { "type": "integer", "minimum": 0, "maximum": 255 }
                    },
                    "required": ["Fixed"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "Rgb": {
                            "type": "array",
                            "items": { "type": "integer", "minimum": 0, "maximum": 255 },
                            "minItems": 3,
                            "maxItems": 3
                        }
                    },
                    "required": ["Rgb"],
                    "additionalProperties": false
                }
            ]
        })
    };
}

// Re-export the macro for use in sibling modules (e.g., experimental.rs).
pub(crate) use color_prop;

/// Color configuration for syntax highlighting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
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
                        "comment": color_prop!("Color for comments"),
                        "string": color_prop!("Color for string literals"),
                        "number": color_prop!("Color for numeric literals"),
                        "keyword": color_prop!("Color for keywords"),
                        "constant": color_prop!("Color for constants (TRUE, FALSE, NULL, NA, etc.)"),
                        "operator": color_prop!("Color for operators"),
                        "punctuation": color_prop!("Color for punctuation"),
                        "identifier": color_prop!("Color for identifiers")
                    }
                },
                "meta": {
                    "type": "object",
                    "description": "Colors for meta commands",
                    "properties": {
                        "command": color_prop!("Color for meta command lines")
                    }
                },
                "prompt": {
                    "type": "object",
                    "description": "Colors for prompts",
                    "properties": {
                        "main": color_prop!("Color for the main R prompt"),
                        "continuation": color_prop!("Color for the continuation prompt"),
                        "shell": color_prop!("Color for the shell mode prompt"),
                        "indicator": color_prop!("Color for the mode indicator text"),
                        "status": {
                            "type": "object",
                            "description": "Colors for command status indicator",
                            "properties": {
                                "success": color_prop!("Color for success status"),
                                "error": color_prop!("Color for error status")
                            }
                        },
                        "vi": {
                            "type": "object",
                            "description": "Colors for vi mode indicator",
                            "properties": {
                                "insert": color_prop!("Color for vi insert mode"),
                                "normal": color_prop!("Color for vi normal mode"),
                                "non_vi": color_prop!("Color for non-vi modes (Emacs, etc.)")
                            }
                        }
                    }
                }
            }
        })
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
            insert: Color::LightGreen,
            normal: Color::LightYellow,
            non_vi: Color::Default,
        }
    }
}

