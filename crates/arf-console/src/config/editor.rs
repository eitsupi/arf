//! Editor configuration.

use crokey::KeyCombination;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// Editing mode for the line editor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EditorMode {
    /// Emacs-style keybindings (default).
    Emacs,
    /// Vi-style keybindings.
    Vi,
}

impl fmt::Display for EditorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditorMode::Emacs => write!(f, "emacs"),
            EditorMode::Vi => write!(f, "vi"),
        }
    }
}

/// Auto-suggestions mode for history-based hints.
///
/// This controls the fish/nushell-style autosuggestions that appear as you type.
#[derive(Debug, Clone, Copy, PartialEq, Default, JsonSchema)]
#[schemars(schema_with = "auto_suggestions_schema")]
pub enum AutoSuggestions {
    /// Disable suggestions entirely.
    None,
    /// Show suggestions from all history (default).
    #[default]
    All,
    /// Show suggestions only from history entries recorded in the current directory.
    ///
    /// Falls back to all history if no matches found in current directory.
    Cwd,
}

/// Custom JSON schema for AutoSuggestions that accepts both boolean and string values.
fn auto_suggestions_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "description": "History-based autosuggestions mode. Accepts boolean (true/false) or string (\"none\", \"all\", \"cwd\").",
        "oneOf": [
            {
                "type": "boolean",
                "description": "true = show all history, false = disable suggestions"
            },
            {
                "type": "string",
                "enum": ["none", "all", "cwd"],
                "description": "none = disable, all = all history, cwd = current directory only"
            }
        ],
        "default": "all"
    })
}

impl fmt::Display for AutoSuggestions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutoSuggestions::None => write!(f, "none"),
            AutoSuggestions::All => write!(f, "all"),
            AutoSuggestions::Cwd => write!(f, "cwd"),
        }
    }
}

// Custom serialization to support both bool and string values.
// Serialize as string (lowercase enum name).
impl Serialize for AutoSuggestions {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

// Custom deserialization to support both bool and string values.
// - true -> All
// - false -> None
// - "none" -> None
// - "all" -> All
// - "cwd" -> Cwd
impl<'de> Deserialize<'de> for AutoSuggestions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct AutoSuggestionsVisitor;

        impl Visitor<'_> for AutoSuggestionsVisitor {
            type Value = AutoSuggestions;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a boolean or string (\"none\", \"all\", \"cwd\")")
            }

            fn visit_bool<E>(self, value: bool) -> Result<AutoSuggestions, E>
            where
                E: de::Error,
            {
                Ok(if value {
                    AutoSuggestions::All
                } else {
                    AutoSuggestions::None
                })
            }

            fn visit_str<E>(self, value: &str) -> Result<AutoSuggestions, E>
            where
                E: de::Error,
            {
                match value.to_lowercase().as_str() {
                    "none" => Ok(AutoSuggestions::None),
                    "all" => Ok(AutoSuggestions::All),
                    "cwd" => Ok(AutoSuggestions::Cwd),
                    _ => Err(de::Error::unknown_variant(value, &["none", "all", "cwd"])),
                }
            }
        }

        deserializer.deserialize_any(AutoSuggestionsVisitor)
    }
}

/// Bottom margin configuration to keep prompt away from terminal bottom.
///
/// Controls how much space to reserve at the bottom of the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Default, JsonSchema)]
#[schemars(schema_with = "bottom_margin_schema")]
pub enum BottomMargin {
    /// Fixed number of lines to reserve at bottom.
    Fixed(u16),
    /// Fraction of terminal height (0.0-1.0).
    Proportional(f32),
    /// Disabled (default) - no bottom margin.
    #[default]
    Disabled,
}

impl fmt::Display for BottomMargin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BottomMargin::Fixed(n) => write!(f, "fixed({})", n),
            BottomMargin::Proportional(v) => write!(f, "proportional({})", v),
            BottomMargin::Disabled => write!(f, "disabled"),
        }
    }
}

/// Custom JSON schema for BottomMargin.
fn bottom_margin_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "description": "Bottom margin to keep prompt away from terminal bottom. Can be a string \"disabled\" or an object with type and value.",
        "oneOf": [
            {
                "type": "string",
                "enum": ["disabled"],
                "description": "Disabled (default) - no bottom margin"
            },
            {
                "type": "object",
                "description": "Fixed number of lines to reserve at bottom",
                "properties": {
                    "fixed": {
                        "type": "integer",
                        "description": "Number of lines to reserve at bottom (0 to terminal height)",
                        "minimum": 0
                    }
                },
                "required": ["fixed"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "Fraction of terminal height to reserve",
                "properties": {
                    "proportional": {
                        "type": "number",
                        "description": "Fraction of terminal height (0.0 = top, 1.0 = bottom/disabled)",
                        "minimum": 0.0,
                        "maximum": 1.0
                    }
                },
                "required": ["proportional"],
                "additionalProperties": false
            }
        ],
        "default": "disabled"
    })
}

// Serialize BottomMargin - can be string "disabled" or object with fixed/proportional.
impl Serialize for BottomMargin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            BottomMargin::Disabled => serializer.serialize_str("disabled"),
            BottomMargin::Fixed(n) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("fixed", n)?;
                map.end()
            }
            BottomMargin::Proportional(v) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("proportional", v)?;
                map.end()
            }
        }
    }
}

// Deserialize BottomMargin - accepts "disabled" string or object with fixed/proportional.
impl<'de> Deserialize<'de> for BottomMargin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        struct BottomMarginVisitor;

        impl<'de> Visitor<'de> for BottomMarginVisitor {
            type Value = BottomMargin;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(
                    "a string \"disabled\" or an object with \"fixed\" or \"proportional\" key",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<BottomMargin, E>
            where
                E: de::Error,
            {
                match value.to_lowercase().as_str() {
                    "disabled" => Ok(BottomMargin::Disabled),
                    _ => Err(de::Error::unknown_variant(value, &["disabled"])),
                }
            }

            fn visit_map<M>(self, mut map: M) -> Result<BottomMargin, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut fixed: Option<u16> = None;
                let mut proportional: Option<f32> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "fixed" => {
                            if fixed.is_some() {
                                return Err(de::Error::duplicate_field("fixed"));
                            }
                            fixed = Some(map.next_value()?);
                        }
                        "proportional" => {
                            if proportional.is_some() {
                                return Err(de::Error::duplicate_field("proportional"));
                            }
                            proportional = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(de::Error::unknown_field(&key, &["fixed", "proportional"]));
                        }
                    }
                }

                match (fixed, proportional) {
                    (Some(n), None) => Ok(BottomMargin::Fixed(n)),
                    (None, Some(v)) => Ok(BottomMargin::Proportional(v)),
                    (None, None) => Err(de::Error::missing_field("fixed or proportional")),
                    (Some(_), Some(_)) => Err(de::Error::custom(
                        "cannot specify both fixed and proportional",
                    )),
                }
            }
        }

        deserializer.deserialize_any(BottomMarginVisitor)
    }
}

/// Editor configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EditorConfig {
    /// Editing mode: "emacs" or "vi".
    pub mode: EditorMode,
    /// Auto-close brackets and quotes.
    pub auto_match: bool,
    /// Highlight matching bracket when cursor is on a bracket.
    pub highlight_matching_bracket: bool,
    /// History-based autosuggestions mode (fish/nushell style).
    ///
    /// String values: `"none"`, `"all"`, `"cwd"`
    /// Boolean values: `false` (= none), `true` (= all)
    ///
    /// - `none`: Disable suggestions
    /// - `all`: Show suggestions from all history (default)
    /// - `cwd`: Show suggestions only from current directory history
    ///
    /// Suggestions appear grayed out and can be accepted with right arrow.
    pub auto_suggestions: AutoSuggestions,
    /// Keyboard shortcuts that insert text.
    /// Format: "modifier-key" = "text to insert"
    /// Examples: "alt-hyphen" = " <- ", "alt-p" = " |> "
    #[serde(default = "default_key_map")]
    #[schemars(schema_with = "key_map_schema")]
    pub key_map: BTreeMap<KeyCombination, String>,
    /// Bottom margin to keep prompt away from terminal bottom.
    ///
    /// - `disabled`: No margin (default)
    /// - `{ fixed = 10 }`: Reserve 10 lines at bottom
    /// - `{ proportional = 0.5 }`: Reserve bottom 50% of terminal
    #[serde(default)]
    pub bottom_margin: BottomMargin,
}

fn default_key_map() -> BTreeMap<KeyCombination, String> {
    let mut map = BTreeMap::new();
    // Alt+- for assignment operator
    if let Ok(key) = "alt-hyphen".parse() {
        map.insert(key, " <- ".to_string());
    }
    // Alt+P for pipe operator (P = Pipe, avoids IDE conflicts with Ctrl+Shift+M)
    if let Ok(key) = "alt-p".parse() {
        map.insert(key, " |> ".to_string());
    }
    map
}

fn key_map_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "description": "Keyboard shortcuts that insert text. Keys are modifier-key combinations (e.g., 'alt-hyphen', 'ctrl-shift-m'). Values are the text to insert.",
        "additionalProperties": {
            "type": "string"
        },
        "examples": [{
            "alt-hyphen": " <- ",
            "alt-p": " |> "
        }]
    })
}

impl Default for EditorConfig {
    fn default() -> Self {
        EditorConfig {
            mode: EditorMode::Emacs,
            auto_match: true,
            highlight_matching_bracket: false,
            auto_suggestions: AutoSuggestions::All,
            key_map: default_key_map(),
            bottom_margin: BottomMargin::default(),
        }
    }
}
