//! Configuration management following XDG Base Directory specification.

mod colors;
mod completion;
mod editor;
mod experimental;
mod history;
mod prompt;
mod reprex;
mod startup;

pub use colors::{ColorsConfig, MetaColorConfig, RColorConfig, StatusColorConfig};
pub use completion::CompletionConfig;
pub use editor::EditorConfig;
pub use experimental::{ExperimentalConfig, HistoryForgetConfig};
pub use history::HistoryConfig;
#[allow(unused_imports)] // StatusSymbol is part of public API for programmatic StatusConfig construction
pub use prompt::{Indicators, ModeIndicatorPosition, PromptConfig, StatusConfig, StatusSymbol};
pub use experimental::SpinnerConfig;
pub use reprex::ReprexConfig;
pub use startup::{RSource, RSourceMode, RSourceStatus, StartupConfig};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Application name for XDG directories.
const APP_NAME: &str = "arf";

/// Main configuration structure.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    pub startup: StartupConfig,
    pub editor: EditorConfig,
    pub prompt: PromptConfig,
    pub completion: CompletionConfig,
    pub history: HistoryConfig,
    pub reprex: ReprexConfig,
    pub colors: ColorsConfig,
    #[serde(default)]
    pub experimental: ExperimentalConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            startup: StartupConfig::default(),
            editor: EditorConfig::default(),
            prompt: PromptConfig::default(),
            completion: CompletionConfig::default(),
            history: HistoryConfig::default(),
            reprex: ReprexConfig::default(),
            colors: ColorsConfig::default(),
            experimental: ExperimentalConfig::default(),
        }
    }
}

/// Get the XDG config directory for this application.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join(APP_NAME))
}

/// Get the XDG data directory for this application.
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|p| p.join(APP_NAME))
}

/// Get the XDG cache directory for this application.
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join(APP_NAME))
}

/// Get the path to the config file.
pub fn config_file_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("arf.toml"))
}

/// Get the history directory path.
///
/// History files are stored in a subdirectory: `~/.local/share/arf/history/`
/// - R mode: `history/r.db`
/// - Shell mode: `history/shell.db`
pub fn history_dir() -> Option<PathBuf> {
    data_dir().map(|p| p.join("history"))
}

/// Load configuration from file, or return defaults if not found.
pub fn load_config() -> Config {
    let Some(config_path) = config_file_path() else {
        return Config::default();
    };

    if !config_path.exists() {
        return Config::default();
    }

    match fs::read_to_string(&config_path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

/// Load configuration from a specific path.
pub fn load_config_from_path(path: &std::path::Path) -> Config {
    if !path.exists() {
        log::warn!("Config file not found: {:?}", path);
        return Config::default();
    }

    match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(e) => {
            log::warn!("Failed to read config file: {}", e);
            Config::default()
        }
    }
}

/// Generate default configuration as a TOML string with comments.
pub fn generate_default_config() -> String {
    let config = Config::default();
    let toml_content = toml::to_string_pretty(&config).expect("Failed to serialize default config");

    // Add Tombi Schema Document Directive on the first line
    // See: https://tombi-toml.github.io/tombi/docs/comment-directive/schema-document-directive/
    let header = r#"#:schema https://raw.githubusercontent.com/eitsupi/arf/main/artifacts/arf.schema.json
# arf configuration file
#
# Documentation: https://github.com/eitsupi/arf

"#;

    format!("{}{}", header, toml_content)
}

/// Initialize a default configuration file at the XDG config location.
///
/// Returns the path where the config was written.
pub fn init_config(force: bool) -> anyhow::Result<std::path::PathBuf> {
    let config_path = config_file_path()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

    // Check if file already exists
    if config_path.exists() && !force {
        anyhow::bail!(
            "Configuration file already exists at: {}\nUse --force to overwrite.",
            config_path.display()
        );
    }

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Generate and write config
    let content = generate_default_config();
    fs::write(&config_path, content)?;

    Ok(config_path)
}

/// Ensure all XDG directories exist.
pub fn ensure_directories() -> anyhow::Result<()> {
    if let Some(dir) = config_dir() {
        fs::create_dir_all(&dir)?;
    }
    if let Some(dir) = data_dir() {
        fs::create_dir_all(&dir)?;
    }
    if let Some(dir) = cache_dir() {
        fs::create_dir_all(&dir)?;
    }
    Ok(())
}

/// Schema generation for configuration.
#[allow(dead_code)]
pub mod schema {
    use super::Config;
    use schemars::schema_for;
    use std::path::PathBuf;

    /// Root directory for artifacts (relative to crate root).
    const ARTIFACTS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../artifacts");

    /// Generate JSON schema for the configuration.
    pub fn generate_schema() -> String {
        let schema = schema_for!(Config);
        serde_json::to_string_pretty(&schema).expect("Failed to serialize schema")
    }

    /// Get the path to the schema file.
    pub fn schema_path() -> PathBuf {
        PathBuf::from(ARTIFACTS_DIR).join("arf.schema.json")
    }

    /// Write the schema to the artifacts directory.
    pub fn write_schema() -> std::io::Result<()> {
        let schema = generate_schema();
        let path = schema_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crokey::KeyCombination;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.editor.auto_match, "auto_match should be enabled by default");
        assert_eq!(config.editor.mode, "emacs");
        assert!(matches!(
            config.startup.r_source,
            RSource::Mode(RSourceMode::Auto)
        ));
        assert!(config.startup.show_banner);
    }

    #[test]
    fn test_parse_config_with_auto_match_enabled() {
        let toml_str = r#"
[editor]
auto_match = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.editor.auto_match);
    }

    #[test]
    fn test_parse_config_with_auto_match_disabled() {
        let toml_str = r#"
[editor]
auto_match = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.editor.auto_match);
    }

    #[test]
    fn test_parse_startup_section_config() {
        let toml_str = r##"
[startup]
r_source = "rig"
show_banner = false

[editor]
mode = "vi"
auto_match = false

[prompt]
format = "R> "
continuation = ".. "

[completion]
enabled = true
timeout_ms = 100

[reprex]
enabled = true
comment = "# "
autoformat = true
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            config.startup.r_source,
            RSource::Mode(RSourceMode::Rig)
        ));
        assert!(!config.startup.show_banner);
        assert_eq!(config.editor.mode, "vi");
        assert!(!config.editor.auto_match);
        assert_eq!(config.prompt.format, "R> ");
        assert!(config.reprex.enabled);
        assert!(config.reprex.autoformat);
    }

    #[test]
    fn test_parse_new_key_map_config() {
        let toml_str = r#"
[editor]
mode = "emacs"

[editor.key_map]
"alt-hyphen" = " <- "
"ctrl-shift-m" = " |> "
"alt-=" = " == "
"#;
        let config: Config = toml::from_str(toml_str).unwrap();

        let alt_hyphen: KeyCombination = "alt-hyphen".parse().unwrap();
        let ctrl_shift_m: KeyCombination = "ctrl-shift-m".parse().unwrap();
        let alt_eq: KeyCombination = "alt-=".parse().unwrap();

        assert_eq!(config.editor.key_map.get(&alt_hyphen), Some(&" <- ".to_string()));
        assert_eq!(config.editor.key_map.get(&ctrl_shift_m), Some(&" |> ".to_string()));
        assert_eq!(config.editor.key_map.get(&alt_eq), Some(&" == ".to_string()));
    }

    #[test]
    fn test_default_key_map() {
        let config = Config::default();

        let alt_hyphen: KeyCombination = "alt-hyphen".parse().unwrap();
        let alt_p: KeyCombination = "alt-p".parse().unwrap();

        assert_eq!(config.editor.key_map.get(&alt_hyphen), Some(&" <- ".to_string()));
        assert_eq!(config.editor.key_map.get(&alt_p), Some(&" |> ".to_string()));
    }

    #[test]
    fn test_default_mode_indicator() {
        let config = Config::default();
        assert_eq!(config.prompt.mode_indicator, ModeIndicatorPosition::Prefix);
        assert_eq!(config.prompt.indicators.reprex, "[reprex] ");
        assert_eq!(config.prompt.indicators.autoformat, "[format] ");
    }

    #[test]
    fn test_parse_mode_indicator_suffix() {
        let toml_str = r#"
[prompt]
mode_indicator = "suffix"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.prompt.mode_indicator, ModeIndicatorPosition::Suffix);
    }

    #[test]
    fn test_parse_mode_indicator_none() {
        let toml_str = r#"
[prompt]
mode_indicator = "none"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.prompt.mode_indicator, ModeIndicatorPosition::None);
    }

    #[test]
    fn test_parse_history_disabled() {
        let toml_str = r#"
[history]
disabled = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.history.disabled);
    }

    #[test]
    fn test_default_history_forget_config() {
        let config = Config::default();
        assert!(!config.experimental.history_forget.enabled);
        assert_eq!(config.experimental.history_forget.delay, 2);
        assert!(!config.experimental.history_forget.on_exit_only);
    }

    #[test]
    fn test_parse_history_forget_config() {
        let toml_str = r#"
[experimental.history_forget]
enabled = true
delay = 5
on_exit_only = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.experimental.history_forget.enabled);
        assert_eq!(config.experimental.history_forget.delay, 5);
        assert!(config.experimental.history_forget.on_exit_only);
    }

    #[test]
    fn test_parse_r_source_auto() {
        let toml_str = r#"
[startup]
r_source = "auto"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            config.startup.r_source,
            RSource::Mode(RSourceMode::Auto)
        ));
    }

    #[test]
    fn test_parse_r_source_rig() {
        let toml_str = r#"
[startup]
r_source = "rig"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            config.startup.r_source,
            RSource::Mode(RSourceMode::Rig)
        ));
    }

    #[test]
    fn test_parse_r_source_path() {
        let toml_str = r#"
[startup]
r_source = { path = "/opt/R/4.5.2" }
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        match &config.startup.r_source {
            RSource::Path { path } => {
                assert_eq!(path, &PathBuf::from("/opt/R/4.5.2"));
            }
            _ => panic!("Expected RSource::Path"),
        }
    }

    #[test]
    fn test_parse_r_source_default_when_omitted() {
        let toml_str = r#"
[startup]
show_banner = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            config.startup.r_source,
            RSource::Mode(RSourceMode::Auto)
        ));
    }

    #[test]
    fn test_generate_default_config() {
        let config_str = generate_default_config();

        // Should have Tombi Schema Document Directive on first line
        assert!(config_str.starts_with("#:schema https://raw.githubusercontent.com/eitsupi/arf/main/artifacts/arf.schema.json"));

        // Should be valid TOML
        let parsed: Config = toml::from_str(&config_str).expect("Generated config should be valid TOML");

        // Should have default values
        assert!(matches!(
            parsed.startup.r_source,
            RSource::Mode(RSourceMode::Auto)
        ));
        assert!(parsed.startup.show_banner);
        assert_eq!(parsed.editor.mode, "emacs");
    }

    #[test]
    fn test_generate_default_config_has_new_structure() {
        let config_str = generate_default_config();

        // Should have [startup] section with r_source and show_banner
        assert!(config_str.contains("[startup]"), "Should have [startup] section");
        assert!(config_str.contains("r_source = "), "Should have r_source in startup section");
        assert!(config_str.contains("show_banner = "), "Should have show_banner in startup section");

        // Should NOT have old sections
        assert!(!config_str.contains("[general]"), "Should NOT have [general] section");
        assert!(!config_str.contains("[shortcuts]"), "Should NOT have [shortcuts] section");
        assert!(!config_str.contains("[formatter]"), "Should NOT have [formatter] section");

        // Should have new sections
        assert!(config_str.contains("[editor]"), "Should have [editor] section");
        assert!(config_str.contains("[reprex]"), "Should have [reprex] section");
    }

    mod schema_tests {
        use crate::config::schema::{generate_schema, schema_path, write_schema};

        #[test]
        fn test_schema_snapshot() {
            let schema = generate_schema();
            insta::assert_snapshot!("config_schema", schema);
        }

        #[test]
        fn test_schema_matches_artifact() {
            let schema = generate_schema();
            let path = schema_path();

            // If the artifact file exists, verify it matches the generated schema
            if path.exists() {
                let contents = std::fs::read_to_string(&path)
                    .expect("Failed to read schema file");
                assert_eq!(
                    schema, contents,
                    "Schema file is out of date. Run the generate_schema_file test to update."
                );
            }
        }

        /// Generate the schema file in artifacts/.
        /// Run with: cargo test -p arf-console generate_schema_file -- --ignored
        #[test]
        #[ignore]
        fn generate_schema_file() {
            write_schema().expect("Failed to write schema file");
            println!("Schema written to {:?}", schema_path());
        }

        #[test]
        fn test_schema_is_valid_json() {
            let schema = generate_schema();
            let parsed: serde_json::Value =
                serde_json::from_str(&schema).expect("Schema should be valid JSON");

            // Verify it has expected top-level fields
            assert!(parsed.get("$schema").is_some(), "Schema should have $schema field");
            assert!(parsed.get("title").is_some(), "Schema should have title field");
            assert!(parsed.get("type").is_some(), "Schema should have type field");
            assert!(parsed.get("properties").is_some(), "Schema should have properties field");
        }

        #[test]
        fn test_schema_has_new_structure() {
            let schema = generate_schema();
            let parsed: serde_json::Value =
                serde_json::from_str(&schema).expect("Schema should be valid JSON");

            let properties = parsed.get("properties").expect("Schema should have properties");

            // Should have startup section (contains r_version and show_banner)
            assert!(properties.get("startup").is_some(), "Schema should have startup section");

            // Should have other sections
            assert!(properties.get("editor").is_some(), "Schema should have editor section");
            assert!(properties.get("prompt").is_some(), "Schema should have prompt section");
            assert!(properties.get("completion").is_some(), "Schema should have completion section");
            assert!(properties.get("reprex").is_some(), "Schema should have reprex section");
            assert!(properties.get("experimental").is_some(), "Schema should have experimental section");

            // Should NOT have legacy sections or top-level fields that moved to startup
            assert!(properties.get("general").is_none(), "Schema should NOT have general section");
            assert!(properties.get("r_version").is_none(), "r_version should be in startup section, not top-level");
            assert!(properties.get("show_banner").is_none(), "show_banner should be in startup section, not top-level");
            assert!(properties.get("shortcuts").is_none(), "Schema should NOT have shortcuts section");
            assert!(properties.get("formatter").is_none(), "Schema should NOT have formatter section");
        }
    }
}
