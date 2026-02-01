//! Editor configuration.

use crokey::KeyCombination;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Editor configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EditorConfig {
    /// Editing mode: "emacs" or "vi".
    pub mode: String,
    /// Auto-close brackets and quotes.
    pub auto_match: bool,
    /// Show history-based autosuggestions (fish/nushell style).
    /// Suggestions appear grayed out and can be accepted with right arrow.
    pub autosuggestion: bool,
    /// Keyboard shortcuts that insert text.
    /// Format: "modifier-key" = "text to insert"
    /// Examples: "alt-hyphen" = " <- ", "alt-p" = " |> "
    #[serde(
        default = "default_key_map",
        serialize_with = "serialize_key_map_sorted"
    )]
    #[schemars(schema_with = "key_map_schema")]
    pub key_map: HashMap<KeyCombination, String>,
}

/// Serialize key_map with keys sorted alphabetically for deterministic output.
fn serialize_key_map_sorted<S>(
    map: &HashMap<KeyCombination, String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // Convert to BTreeMap<String, String> for sorted serialization
    let sorted: BTreeMap<String, &String> = map.iter().map(|(k, v)| (k.to_string(), v)).collect();
    sorted.serialize(serializer)
}

fn default_key_map() -> HashMap<KeyCombination, String> {
    let mut map = HashMap::new();
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
            mode: "emacs".to_string(),
            auto_match: true,
            autosuggestion: true,
            key_map: default_key_map(),
        }
    }
}
