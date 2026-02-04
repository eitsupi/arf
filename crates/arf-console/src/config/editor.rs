//! Editor configuration.

use crokey::KeyCombination;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Editing mode for the line editor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EditorMode {
    /// Emacs-style keybindings (default).
    Emacs,
    /// Vi/Vim-style keybindings.
    #[serde(alias = "vim")]
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

/// Editor configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EditorConfig {
    /// Editing mode: "emacs" or "vi".
    pub mode: EditorMode,
    /// Auto-close brackets and quotes.
    pub auto_match: bool,
    /// Show history-based autosuggestions (fish/nushell style).
    /// Suggestions appear grayed out and can be accepted with right arrow.
    pub auto_suggestions: bool,
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
            auto_suggestions: true,
            key_map: default_key_map(),
        }
    }
}
