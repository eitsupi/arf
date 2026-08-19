//! Configuration management following XDG Base Directory specification.

mod colors;
mod completion;
mod editor;
mod experimental;
mod history;
mod ipc;
mod mode;
pub(crate) mod prompt;
mod r;
mod startup;

pub use colors::{ColorsConfig, MetaColorConfig, RColorConfig, StatusColorConfig, ViColorConfig};
pub use completion::CompletionConfig;
pub use editor::{AutoSuggestions, EditorConfig, EditorMode};
pub use experimental::{
    ExperimentalConfig, HistoryForgetConfig, PromptDurationConfig, RSourceOverride, SpinnerConfig,
};
pub use history::{HistoryConfig, HistoryMode};
pub use ipc::IpcConfig;
pub use mode::ModeConfig;
#[allow(unused_imports)]
// StatusSymbol is part of public API for programmatic StatusConfig construction
pub use prompt::{
    Indicators, ModeIndicatorPosition, PromptConfig, StatusConfig, StatusSymbol, ViConfig,
};
pub use r::RConfig;
pub use startup::{RSource, RSourceMode, RSourceOverrideInfo, RSourceStatus, StartupConfig};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Status of config file loading, for display in `:info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStatus {
    /// Config loaded successfully (or no config file exists).
    Ok,
    /// Config file could not be read (e.g., permission denied).
    ReadError,
    /// Config file has TOML syntax or schema errors.
    ParseError,
}

/// Error type for configuration loading failures.
#[derive(Debug)]
pub enum ConfigLoadError {
    /// Failed to read the config file from disk.
    Read {
        source: std::io::Error,
        path: PathBuf,
    },
    /// Failed to parse the TOML content.
    Parse {
        source: toml::de::Error,
        path: PathBuf,
    },
}

/// Configuration metadata collected while loading the file contents.
#[derive(Debug)]
pub(crate) struct ConfigLoadProvenance {
    pub(crate) path: PathBuf,
    pub(crate) startup_r_source_present: bool,
    pub(crate) history_migration_warning: Option<String>,
}

impl std::fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigLoadError::Read { source, path } => {
                write!(
                    f,
                    "Failed to read config file {}: {}",
                    path.display(),
                    source
                )
            }
            ConfigLoadError::Parse { source, path } => {
                write!(
                    f,
                    "Failed to parse config file {}: {}",
                    path.display(),
                    source
                )
            }
        }
    }
}

impl std::error::Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigLoadError::Read { source, .. } => Some(source),
            ConfigLoadError::Parse { source, .. } => Some(source),
        }
    }
}

/// Application name for XDG directories.
const APP_NAME: &str = "arf";

/// Main configuration structure.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub startup: StartupConfig,
    pub editor: EditorConfig,
    pub prompt: PromptConfig,
    pub completion: CompletionConfig,
    pub history: HistoryConfig,
    /// IPC evaluation policy configuration.
    pub ipc: IpcConfig,
    /// R runtime configuration.
    pub r: RConfig,
    /// Mode-specific static configuration (not changeable at runtime).
    /// For initial mode state (enabled/disabled), see `startup.mode`.
    pub mode: ModeConfig,
    pub colors: ColorsConfig,
    #[serde(default)]
    pub experimental: ExperimentalConfig,
    #[serde(skip)]
    pub(crate) history_migration_warning: Option<String>,
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

/// Resolve the on-disk history directory for a configured mode.
///
/// Volatile history deliberately has no path: it must not expose, read, or
/// create the persistent XDG directory. Persistent mode uses its explicit
/// directory when present and otherwise the XDG default.
pub fn history_dir_for_mode(mode: &HistoryMode) -> Option<PathBuf> {
    match mode {
        HistoryMode::Persistent { .. } => mode.persistent_dir().cloned().or_else(history_dir),
        HistoryMode::Volatile => None,
    }
}

/// Mask home directory in path with `~` for privacy.
///
/// Replaces the user's home directory prefix with `~` to avoid displaying
/// usernames in paths. Works on both Unix and Windows (PowerShell supports `~`).
/// Uses platform-native path separators.
pub fn mask_home_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(stripped) = path.strip_prefix(&home)
    {
        return format!("~{}{}", std::path::MAIN_SEPARATOR, stripped.display());
    }
    path.display().to_string()
}

/// Load configuration from the default XDG config file.
///
/// Returns `Ok(Config::default())` if no config file exists.
/// Returns `Err` if the file exists but cannot be read or parsed.
pub fn load_config() -> Result<Config, ConfigLoadError> {
    let Some(config_path) = config_file_path() else {
        return Ok(Config::default());
    };

    if !config_path.exists() {
        return Ok(Config::default());
    }

    load_config_from_path(&config_path)
}

/// Load configuration from a specific path.
///
/// Returns `Ok(Config::default())` if the file does not exist.
/// Returns `Err` if the file exists but cannot be read or parsed.
pub fn load_config_from_path(path: &std::path::Path) -> Result<Config, ConfigLoadError> {
    load_config_from_path_with_provenance(path).map(|(config, _)| config)
}

/// Load configuration and report metadata from the same file read.
pub(crate) fn load_config_from_path_with_provenance(
    path: &std::path::Path,
) -> Result<(Config, Option<ConfigLoadProvenance>), ConfigLoadError> {
    if !path.exists() {
        log::warn!("Config file not found: {:?}", path);
        return Ok((Config::default(), None));
    }

    let content = fs::read_to_string(path).map_err(|e| ConfigLoadError::Read {
        source: e,
        path: path.to_path_buf(),
    })?;

    let document = toml::from_str::<toml::Value>(&content).map_err(|e| ConfigLoadError::Parse {
        source: e,
        path: path.to_path_buf(),
    })?;
    let startup_r_source_present = document
        .get("startup")
        .and_then(|startup| startup.get("r_source"))
        .is_some();
    let mut config = toml::from_str::<Config>(&content).map_err(|e| ConfigLoadError::Parse {
        source: e,
        path: path.to_path_buf(),
    })?;
    config.history_migration_warning = history_migration_warning(&document);
    let migration_warning = config.history_migration_warning.clone();

    Ok((
        config,
        Some(ConfigLoadProvenance {
            path: path.to_path_buf(),
            startup_r_source_present,
            history_migration_warning: migration_warning,
        }),
    ))
}

fn history_migration_warning(document: &toml::Value) -> Option<String> {
    let history = document.get("history")?.as_table()?;
    let has_mode = history.contains_key("mode");
    let disabled = history.get("disabled").and_then(toml::Value::as_bool);
    let has_legacy_dir = history.contains_key("dir");
    match (has_mode, has_legacy_dir, disabled) {
        (true, _, Some(_)) => Some("Config key history.disabled is deprecated and ignored because history.mode is set; use history.mode only.".to_string()),
        (false, true, Some(true)) => Some("Config keys history.disabled and history.dir are deprecated; use history.mode = \"volatile\" instead.".to_string()),
        (false, true, Some(false)) => Some("Config keys history.disabled and history.dir are deprecated; use persistent history.mode = { dir = \"...\" } instead.".to_string()),
        (false, true, None) => Some("Config key history.dir is deprecated; use history.mode = { dir = \"...\" } instead.".to_string()),
        (false, false, Some(true)) => Some("Config key history.disabled is deprecated; use history.mode = \"volatile\" instead.".to_string()),
        (false, false, Some(false)) => Some("Config key history.disabled is deprecated; use history.mode = \"persistent\" instead.".to_string()),
        _ => None,
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
        assert!(
            config.editor.auto_match,
            "auto_match should be enabled by default"
        );
        assert_eq!(config.editor.mode, EditorMode::Emacs);
        assert!(matches!(
            config.startup.r_source,
            RSource::Mode(RSourceMode::Auto)
        ));
        assert!(config.startup.show_banner);
    }

    #[test]
    fn test_default_r_auto_width() {
        let config = Config::default();
        assert!(
            config.r.auto_width,
            "auto_width should be enabled by default"
        );
    }

    #[test]
    fn test_parse_r_auto_width_disabled() {
        let toml_str = r#"
[r]
auto_width = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.r.auto_width);
    }

    #[test]
    fn test_parse_r_auto_width_default_when_omitted() {
        let toml_str = r#"
[editor]
mode = "vi"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(
            config.r.auto_width,
            "auto_width should default to true when [r] section is omitted"
        );
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

[startup.mode]
reprex = true
autoformat = true

[editor]
mode = "vi"
auto_match = false

[prompt]
format = "R> "
continuation = ".. "

[completion]
enabled = true
timeout_ms = 100

[mode.reprex]
comment = "# "
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            config.startup.r_source,
            RSource::Mode(RSourceMode::Rig)
        ));
        assert!(!config.startup.show_banner);
        assert_eq!(config.editor.mode, EditorMode::Vi);
        assert!(!config.editor.auto_match);
        assert_eq!(config.prompt.format, "R> ");
        assert!(config.startup.mode.reprex);
        assert!(config.startup.mode.autoformat);
        assert_eq!(config.mode.reprex.comment, "# ");
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

        assert_eq!(
            config.editor.key_map.get(&alt_hyphen),
            Some(&" <- ".to_string())
        );
        assert_eq!(
            config.editor.key_map.get(&ctrl_shift_m),
            Some(&" |> ".to_string())
        );
        assert_eq!(
            config.editor.key_map.get(&alt_eq),
            Some(&" == ".to_string())
        );
    }

    #[test]
    fn test_default_key_map() {
        let config = Config::default();

        let alt_hyphen: KeyCombination = "alt-hyphen".parse().unwrap();
        let alt_p: KeyCombination = "alt-p".parse().unwrap();

        assert_eq!(
            config.editor.key_map.get(&alt_hyphen),
            Some(&" <- ".to_string())
        );
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
    fn test_parse_history_mode_volatile() {
        let toml_str = r#"
[history]
mode = "volatile"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(config.history.mode, HistoryMode::Volatile));
    }

    #[test]
    fn test_parse_history_mode_with_directory_object() {
        let config: Config =
            toml::from_str("[history]\nmode = { dir = \"/custom/history\" }\n").unwrap();
        assert!(matches!(
            config.history.mode,
            HistoryMode::Persistent { dir: Some(ref dir) }
                if dir == std::path::Path::new("/custom/history")
        ));

        let serialized = toml::to_string(&config).unwrap();
        assert!(
            serialized.contains("[history.mode]")
                && serialized.contains("dir = \"/custom/history\""),
            "serialized history config used an unexpected TOML shape: {serialized}"
        );
        assert!(!serialized.contains("[history]\ndir = "));
    }

    #[test]
    fn test_history_mode_object_requires_dir_and_rejects_unknown_fields() {
        assert!(toml::from_str::<Config>("[history]\nmode = {}\n").is_err());
        assert!(
            toml::from_str::<Config>("[history]\nmode = { dir = \"/tmp\", extra = true }\n")
                .is_err()
        );
    }

    #[test]
    fn test_history_mode_rejects_legacy_dir_when_explicit() {
        let result =
            toml::from_str::<Config>("[history]\nmode = \"persistent\"\ndir = \"/tmp/history\"\n");
        assert!(result.is_err());
    }

    #[test]
    fn test_legacy_history_disabled_migrates_with_warning() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("arf.toml");
        fs::write(&path, "[history]\ndisabled = true\n").unwrap();
        let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
        assert!(matches!(config.history.mode, HistoryMode::Volatile));
        assert!(config.history_migration_warning.is_some());
        assert!(provenance.unwrap().history_migration_warning.is_some());
    }

    #[test]
    fn test_legacy_history_disabled_false_preserves_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("arf.toml");
        fs::write(
            &path,
            "[history]\ndisabled = false\ndir = \"/tmp/arf-history\"\n",
        )
        .unwrap();
        let (loaded, provenance) = load_config_from_path_with_provenance(&path).unwrap();
        assert!(
            provenance
                .unwrap()
                .history_migration_warning
                .unwrap()
                .contains("persistent")
        );
        assert!(matches!(
            loaded.history.mode,
            HistoryMode::Persistent { .. }
        ));

        let config: Config =
            toml::from_str("[history]\ndisabled = false\ndir = \"/tmp/arf-history\"\n").unwrap();
        assert!(matches!(
            config.history.mode,
            HistoryMode::Persistent { dir: Some(ref dir) } if dir == std::path::Path::new("/tmp/arf-history")
        ));
    }

    #[test]
    fn test_legacy_history_dir_without_disabled_migrates_to_persistent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("arf.toml");
        fs::write(&path, "[history]\ndir = \"/tmp/arf-history\"\n").unwrap();
        let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
        assert!(matches!(
            config.history.mode,
            HistoryMode::Persistent { dir: Some(ref dir) }
                if dir == std::path::Path::new("/tmp/arf-history")
        ));
        assert!(
            provenance
                .unwrap()
                .history_migration_warning
                .unwrap()
                .contains("deprecated")
        );
    }

    #[test]
    fn test_history_mode_wins_over_legacy_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("arf.toml");
        fs::write(&path, "[history]\nmode = \"volatile\"\ndisabled = false\n").unwrap();
        let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
        assert!(
            provenance
                .unwrap()
                .history_migration_warning
                .unwrap()
                .contains("ignored")
        );
        assert!(matches!(config.history.mode, HistoryMode::Volatile));
    }

    #[test]
    fn test_history_mode_object_wins_over_legacy_disabled() {
        for disabled in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("arf.toml");
            fs::write(
                &path,
                format!(
                    "[history]\nmode = {{ dir = \"/tmp/payload-history\" }}\ndisabled = {disabled}\n"
                ),
            )
            .unwrap();
            let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
            assert!(matches!(
                config.history.mode,
                HistoryMode::Persistent { dir: Some(ref dir) }
                    if dir == std::path::Path::new("/tmp/payload-history")
            ));
            assert!(
                provenance
                    .unwrap()
                    .history_migration_warning
                    .unwrap()
                    .contains("ignored")
            );
        }
    }

    #[test]
    fn test_history_mode_wrong_type_is_parse_error() {
        let result = toml::from_str::<Config>("[history]\nmode = 1\n");
        assert!(result.is_err());
        let result = toml::from_str::<Config>("[history]\ndisabled = \"true\"\n");
        assert!(result.is_err());
    }

    #[test]
    fn test_history_dir_for_mode_never_exposes_a_volatile_path() {
        assert_eq!(
            history_dir_for_mode(&HistoryMode::Volatile),
            None,
            "volatile history must not expose the persistent XDG directory"
        );
        let explicit = std::path::PathBuf::from("/tmp/arf-history");
        assert_eq!(
            history_dir_for_mode(&HistoryMode::Persistent {
                dir: Some(explicit.clone()),
            }),
            Some(explicit)
        );
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
    fn test_auto_suggestions_default_is_all() {
        let config = Config::default();
        assert_eq!(config.editor.auto_suggestions, AutoSuggestions::All);
    }

    #[test]
    fn test_parse_auto_suggestions_none_string() {
        let toml_str = r#"
[editor]
auto_suggestions = "none"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.editor.auto_suggestions, AutoSuggestions::None);
    }

    #[test]
    fn test_parse_auto_suggestions_all_string() {
        let toml_str = r#"
[editor]
auto_suggestions = "all"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.editor.auto_suggestions, AutoSuggestions::All);
    }

    #[test]
    fn test_parse_auto_suggestions_cwd_string() {
        let toml_str = r#"
[editor]
auto_suggestions = "cwd"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.editor.auto_suggestions, AutoSuggestions::Cwd);
    }

    #[test]
    fn test_parse_auto_suggestions_bool_true() {
        let toml_str = r#"
[editor]
auto_suggestions = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.editor.auto_suggestions, AutoSuggestions::All);
    }

    #[test]
    fn test_parse_auto_suggestions_bool_false() {
        let toml_str = r#"
[editor]
auto_suggestions = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.editor.auto_suggestions, AutoSuggestions::None);
    }

    #[test]
    fn test_parse_auto_suggestions_invalid_string() {
        let toml_str = r#"
[editor]
auto_suggestions = "invalid"
"#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown variant"),
            "Error should mention unknown variant: {}",
            err
        );
    }

    #[test]
    fn test_parse_auto_suggestions_string_true_rejected() {
        // String "true" should be rejected (use boolean true instead)
        let toml_str = r#"
[editor]
auto_suggestions = "true"
"#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        assert!(result.is_err(), "String 'true' should be rejected");
    }

    #[test]
    fn test_parse_auto_suggestions_string_false_rejected() {
        // String "false" should be rejected (use boolean false instead)
        let toml_str = r#"
[editor]
auto_suggestions = "false"
"#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        assert!(result.is_err(), "String 'false' should be rejected");
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
    fn test_parse_ipc_eval_allowed_functions() {
        let config: Config = toml::from_str(
            r#"
[ipc.eval]
allowed_functions = ["mean", "stats::median", "+"]
"#,
        )
        .unwrap();
        assert_eq!(
            config.ipc.eval.allowed_functions,
            ["mean", "stats::median", "+"]
        );
        assert!(Config::default().ipc.eval.allowed_functions.is_empty());
    }

    #[test]
    fn test_generate_default_config() {
        let config_str = generate_default_config();

        // Should have Tombi Schema Document Directive on first line
        assert!(config_str.starts_with(
            "#:schema https://raw.githubusercontent.com/eitsupi/arf/main/artifacts/arf.schema.json"
        ));

        // Should be valid TOML
        let parsed: Config =
            toml::from_str(&config_str).expect("Generated config should be valid TOML");

        // Should have default values
        assert!(matches!(
            parsed.startup.r_source,
            RSource::Mode(RSourceMode::Auto)
        ));
        assert!(parsed.startup.show_banner);
        assert_eq!(parsed.editor.mode, EditorMode::Emacs);
    }

    #[test]
    fn test_generate_default_config_has_new_structure() {
        let config_str = generate_default_config();

        // Should have [startup] section with r_source and show_banner
        assert!(
            config_str.contains("[startup]"),
            "Should have [startup] section"
        );
        assert!(
            config_str.contains("r_source = "),
            "Should have r_source in startup section"
        );
        assert!(
            config_str.contains("show_banner = "),
            "Should have show_banner in startup section"
        );

        // Should have [startup.mode] section
        assert!(
            config_str.contains("[startup.mode]"),
            "Should have [startup.mode] section"
        );

        // Should have [mode.reprex] section
        assert!(
            config_str.contains("[mode.reprex]"),
            "Should have [mode.reprex] section"
        );

        // Should NOT have old sections
        assert!(
            !config_str.contains("[general]"),
            "Should NOT have [general] section"
        );
        assert!(
            !config_str.contains("[shortcuts]"),
            "Should NOT have [shortcuts] section"
        );
        assert!(
            !config_str.contains("[formatter]"),
            "Should NOT have [formatter] section"
        );

        // Should have other sections
        assert!(
            config_str.contains("[editor]"),
            "Should have [editor] section"
        );
        assert!(
            config_str.contains("mode = \"persistent\""),
            "History should default to persistent mode"
        );
        assert!(
            !config_str.contains("disabled"),
            "Deprecated history.disabled must not be serialized"
        );
    }

    mod schema_tests {
        use crate::config::schema::{generate_schema, schema_path, write_schema};

        #[test]
        fn test_schema_snapshot() {
            let schema = generate_schema();
            insta::assert_snapshot!("config_schema", schema);
        }

        /// Snapshot test for the default configuration file.
        /// This ensures we notice when the config structure changes.
        #[test]
        fn test_default_config_snapshot() {
            let config = crate::config::generate_default_config();
            insta::assert_snapshot!("default_config", config);
        }

        #[test]
        fn test_schema_matches_artifact() {
            let schema = generate_schema();
            let path = schema_path();

            // If the artifact file exists, verify it matches the generated schema
            if path.exists() {
                let contents = std::fs::read_to_string(&path).expect("Failed to read schema file");
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
            assert!(
                parsed.get("$schema").is_some(),
                "Schema should have $schema field"
            );
            assert!(
                parsed.get("title").is_some(),
                "Schema should have title field"
            );
            assert!(
                parsed.get("type").is_some(),
                "Schema should have type field"
            );
            assert!(
                parsed.get("properties").is_some(),
                "Schema should have properties field"
            );
        }

        #[test]
        fn test_schema_has_new_structure() {
            let schema = generate_schema();
            let parsed: serde_json::Value =
                serde_json::from_str(&schema).expect("Schema should be valid JSON");

            let properties = parsed
                .get("properties")
                .expect("Schema should have properties");

            let history = properties
                .get("history")
                .and_then(|value| value.get("$ref"))
                .and_then(|value| value.as_str())
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
                .and_then(|name| parsed.get("$defs").and_then(|defs| defs.get(name)))
                .expect("Schema should define history");
            let history_properties = history
                .get("properties")
                .expect("History schema should have properties");
            assert!(history_properties.get("disabled").is_none());
            assert!(history_properties.get("dir").is_none());
            let mode = history_properties
                .get("mode")
                .expect("History mode should be present in schema");
            assert!(
                history
                    .get("required")
                    .and_then(|required| required.as_array())
                    .is_none_or(|required| !required.iter().any(|name| name == "mode")),
                "History mode must remain optional for the default persistent mode"
            );
            assert_eq!(mode["default"], "persistent");
            let variants = mode
                .get("oneOf")
                .and_then(|variants| variants.as_array())
                .expect("History mode should have string and object variants");
            assert!(variants.iter().any(|variant| {
                variant["type"] == "string"
                    && variant["enum"]
                        .as_array()
                        .is_some_and(|values| values.iter().any(|value| value == "persistent"))
            }));
            assert!(variants.iter().any(|variant| {
                variant["type"] == "object"
                    && variant["additionalProperties"] == false
                    && variant["required"]
                        .as_array()
                        .is_some_and(|required| required.iter().any(|name| name == "dir"))
            }));

            // Should have startup section (contains r_source, show_banner, mode)
            assert!(
                properties.get("startup").is_some(),
                "Schema should have startup section"
            );

            // Should have mode section (contains reprex static config)
            assert!(
                properties.get("mode").is_some(),
                "Schema should have mode section"
            );

            // Should have other sections
            assert!(
                properties.get("editor").is_some(),
                "Schema should have editor section"
            );
            assert!(
                properties.get("prompt").is_some(),
                "Schema should have prompt section"
            );
            assert!(
                properties.get("completion").is_some(),
                "Schema should have completion section"
            );
            assert!(
                properties.get("experimental").is_some(),
                "Schema should have experimental section"
            );

            // Should NOT have legacy sections or top-level fields that moved
            assert!(
                properties.get("general").is_none(),
                "Schema should NOT have general section"
            );
            assert!(
                properties.get("reprex").is_none(),
                "reprex should be in mode section, not top-level"
            );
            assert!(
                properties.get("r_version").is_none(),
                "r_version should be in startup section, not top-level"
            );
            assert!(
                properties.get("show_banner").is_none(),
                "show_banner should be in startup section, not top-level"
            );
            assert!(
                properties.get("shortcuts").is_none(),
                "Schema should NOT have shortcuts section"
            );
            assert!(
                properties.get("formatter").is_none(),
                "Schema should NOT have formatter section"
            );
        }

        #[test]
        fn test_schema_has_r_source_overrides() {
            let schema = generate_schema();
            let parsed: serde_json::Value =
                serde_json::from_str(&schema).expect("Schema should be valid JSON");

            let experimental = parsed
                .get("$defs")
                .and_then(|defs| defs.get("ExperimentalConfigSchema"))
                .and_then(|schema| schema.get("properties"))
                .expect("Schema should define experimental properties");
            assert!(
                experimental.get("r_source_overrides").is_some(),
                "Schema should have r_source_overrides"
            );
        }
    }

    #[test]
    fn test_mask_home_path_with_home_prefix() {
        // mask_home_path reads HOME indirectly through dirs::home_dir.
        let _guard = crate::test_utils::lock_env();
        if let Some(home) = dirs::home_dir() {
            let test_path = home.join("test").join("file.txt");
            let masked = mask_home_path(&test_path);
            assert!(
                masked.starts_with("~"),
                "Path should start with ~: {}",
                masked
            );
            assert!(
                masked.contains("test"),
                "Path should contain 'test': {}",
                masked
            );
            assert!(
                masked.contains("file.txt"),
                "Path should contain 'file.txt': {}",
                masked
            );
        }
    }

    #[test]
    fn test_mask_home_path_without_home_prefix() {
        // mask_home_path reads HOME indirectly through dirs::home_dir.
        let _guard = crate::test_utils::lock_env();
        let test_path = PathBuf::from("/opt/R/4.5.0");
        let expected = test_path.display().to_string();
        let masked = mask_home_path(&test_path);
        assert_eq!(
            masked, expected,
            "Path without home prefix should be unchanged"
        );
    }

    #[test]
    fn test_mask_home_path_exact_home() {
        // mask_home_path reads HOME indirectly through dirs::home_dir.
        let _guard = crate::test_utils::lock_env();
        if let Some(home) = dirs::home_dir() {
            let masked = mask_home_path(&home);
            // Should be just "~/" or "~\" depending on platform
            assert!(
                masked.starts_with("~"),
                "Home path should start with ~: {}",
                masked
            );
        }
    }

    #[test]
    fn test_load_config_from_path_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[editor\nauto_match = true").unwrap();

        let result = load_config_from_path(&path);
        assert!(result.is_err(), "Invalid TOML should return Err");

        let err = result.unwrap_err();
        assert!(
            matches!(err, ConfigLoadError::Parse { .. }),
            "Should be a ParseError: {:?}",
            err
        );
        let msg = err.to_string();
        assert!(
            msg.contains("bad.toml"),
            "Error should mention the file path: {}",
            msg
        );
    }

    #[test]
    fn test_load_config_from_path_type_error_includes_location() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid-type.toml");
        std::fs::write(
            &path,
            r#"
[editor]
mode = 42
"#,
        )
        .unwrap();

        let error = load_config_from_path(&path).expect_err("invalid config type should fail");
        let message = error.to_string();

        assert!(
            message.contains("line 3, column 8"),
            "Type error should include source location: {message}"
        );
    }

    #[test]
    fn test_load_config_from_path_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("good.toml");
        std::fs::write(
            &path,
            r#"
[editor]
auto_match = false
"#,
        )
        .unwrap();

        let result = load_config_from_path(&path);
        assert!(result.is_ok(), "Valid TOML should return Ok");
        let config = result.unwrap();
        assert!(!config.editor.auto_match);
    }

    #[test]
    fn test_load_config_from_path_not_found() {
        let path = PathBuf::from("/nonexistent/config.toml");
        let result = load_config_from_path(&path);
        assert!(result.is_ok(), "Missing file should return Ok(default)");
        let config = result.unwrap();
        assert!(config.editor.auto_match, "Should be default config");
    }

    #[test]
    fn test_config_load_error_display() {
        let err = ConfigLoadError::Parse {
            source: toml::from_str::<Config>("[bad\n").unwrap_err(),
            path: PathBuf::from("/home/user/.config/arf/arf.toml"),
        };
        let msg = err.to_string();
        assert!(msg.contains("parse"), "Should mention parse: {}", msg);
        assert!(msg.contains("arf.toml"), "Should mention path: {}", msg);
    }

    #[cfg(unix)]
    #[test]
    fn test_load_config_from_path_read_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unreadable.toml");
        std::fs::write(&path, "[editor]\nauto_match = true").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = load_config_from_path(&path);
        assert!(result.is_err(), "Unreadable file should return Err");

        let err = result.unwrap_err();
        assert!(
            matches!(err, ConfigLoadError::Read { .. }),
            "Should be a ReadError: {:?}",
            err
        );

        // Restore permissions so tempdir cleanup succeeds
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}
