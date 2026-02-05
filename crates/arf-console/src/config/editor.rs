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
#[serde(rename_all = "lowercase")]
pub enum AutoSuggestions {
    /// Disable suggestions entirely.
    #[serde(alias = "false")]
    None,
    /// Show suggestions from all history (default).
    #[default]
    #[serde(alias = "true")]
    All,
    /// Show suggestions only from history entries recorded in the current directory.
    ///
    /// Falls back to all history if no matches found in current directory.
    Cwd,
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
                    "none" | "false" => Ok(AutoSuggestions::None),
                    "all" | "true" => Ok(AutoSuggestions::All),
                    "cwd" => Ok(AutoSuggestions::Cwd),
                    _ => Err(de::Error::unknown_variant(value, &["none", "all", "cwd"])),
                }
            }
        }

        deserializer.deserialize_any(AutoSuggestionsVisitor)
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
    /// History-based autosuggestions mode (fish/nushell style).
    ///
    /// - `"none"` or `false`: Disable suggestions
    /// - `"all"` or `true`: Show suggestions from all history (default)
    /// - `"cwd"`: Show suggestions only from current directory history
    ///
    /// Suggestions appear grayed out and can be accepted with right arrow.
    pub auto_suggestions: AutoSuggestions,
    /// Keyboard shortcuts that insert text.
    /// Format: "modifier-key" = "text to insert"
    /// Examples: "alt-hyphen" = " <- ", "alt-p" = " |> "
    #[serde(default = "default_key_map")]
    #[schemars(schema_with = "key_map_schema")]
    pub key_map: BTreeMap<KeyCombination, String>,
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
            auto_suggestions: AutoSuggestions::All,
            key_map: default_key_map(),
        }
    }
}
