//! Configuration management following XDG Base Directory specification.

mod colors;
mod completion;
mod editor;
mod experimental;
mod history;
mod ipc;
pub(crate) mod prompt;
mod r;
mod reprex;
mod startup;

pub use colors::{ColorsConfig, MetaColorConfig, RColorConfig, StatusColorConfig, ViColorConfig};
pub use completion::CompletionConfig;
pub use editor::{AutoSuggestions, EditorConfig, EditorMode};
pub use experimental::{
    ExperimentalConfig, HistoryForgetConfig, PromptDurationConfig, RSourceOverride, SpinnerConfig,
    StaticFormalsMode,
};
#[allow(unused_imports)]
pub use experimental::{HelpViewer, RHelpConfig};
pub use history::{HistoryConfig, HistoryMode};
pub(crate) use history::{HistoryLocationSource, ResolvedHistoryLocation};
pub use ipc::IpcConfig;
#[allow(unused_imports)]
// StatusSymbol is part of public API for programmatic StatusConfig construction
pub use prompt::{
    Indicators, ModeIndicatorPosition, PromptConfig, StatusConfig, StatusSymbol, ViConfig,
};
pub use r::RConfig;
pub use reprex::{FormatterBackend, ReprexConfig, ReprexFormatter};
pub use startup::{
    RSource, RSourceMode, RSourceOverrideInfo, RSourceStatus, ReprexMode, StartupConfig,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
    /// A removed configuration key was found.
    Validation { message: String, path: PathBuf },
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
            ConfigLoadError::Validation { message, path } => {
                write!(f, "Invalid config file {}: {}", path.display(), message)
            }
        }
    }
}

impl std::error::Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigLoadError::Read { source, .. } => Some(source),
            ConfigLoadError::Parse { source, .. } => Some(source),
            ConfigLoadError::Validation { .. } => None,
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
    /// Static reprex configuration.
    pub reprex: ReprexConfig,
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

/// Resolve history mode against the current default directory without probing
/// the filesystem. Volatile mode does not query the default path at all.
pub(crate) fn resolved_history_location(mode: &HistoryMode) -> ResolvedHistoryLocation {
    let default_directory = match mode {
        HistoryMode::Persistent { dir: None } => history_dir(),
        HistoryMode::Persistent { dir: Some(_) } | HistoryMode::Volatile => None,
    };
    history::resolve_history_location(mode, default_directory)
}

/// Apply command-line history overrides to a configured mode.
///
/// Clap supplies `ARF_HISTORY_DIR` through `cli_history_dir`, so the CLI value
/// (whether entered as a flag or environment variable) precedes config.
pub(crate) fn history_mode_with_overrides(
    configured: &HistoryMode,
    cli_history_dir: Option<&Path>,
    no_history: bool,
) -> HistoryMode {
    if no_history {
        HistoryMode::Volatile
    } else if let Some(directory) = cli_history_dir {
        HistoryMode::Persistent {
            dir: Some(directory.to_path_buf()),
        }
    } else {
        configured.clone()
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
    if let Some(message) = removed_reprex_keys(&document) {
        return Err(ConfigLoadError::Validation {
            message,
            path: path.to_path_buf(),
        });
    }
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

fn removed_reprex_keys(document: &toml::Value) -> Option<String> {
    let mut messages = Vec::new();

    if document
        .get("startup")
        .and_then(|value| value.get("mode"))
        .is_some()
    {
        messages.push(r#"[startup.mode] was removed; use [startup] reprex = "off"|"on"|"format""#);
    }
    if document
        .get("mode")
        .and_then(|value| value.get("reprex"))
        .is_some()
    {
        messages.push("[mode.reprex] was removed; use [reprex]");
    }
    if let Some(table) = document.get("reprex").and_then(toml::Value::as_table) {
        if table.contains_key("enabled") {
            messages.push("reprex.enabled was removed; use startup.reprex");
        }
        if table.contains_key("autoformat") {
            messages.push("reprex.autoformat was removed; use startup.reprex");
        }
    }
    if document
        .get("prompt")
        .and_then(|value| value.get("indicators"))
        .and_then(toml::Value::as_table)
        .is_some_and(|table| table.contains_key("autoformat"))
    {
        messages
            .push("prompt.indicators.autoformat was removed; use prompt.indicators.reprex_format");
    }

    (!messages.is_empty()).then(|| messages.join("\n  "))
}

fn history_migration_warning(document: &toml::Value) -> Option<String> {
    let history = document.get("history")?.as_table()?;
    let has_mode = history.contains_key("mode");
    let disabled = history.get("disabled").and_then(toml::Value::as_bool);
    let has_legacy_dir = history.contains_key("dir");
    match (has_mode, has_legacy_dir, disabled) {
        (true, _, Some(_)) => Some("Config key history.disabled is deprecated and ignored because history.mode is set; use history.mode only.".to_string()),
        (false, true, Some(true)) => Some(r#"Config keys history.disabled and history.dir are deprecated; use history.mode = "volatile" instead."#.to_string()),
        (false, true, Some(false)) => Some(r#"Config keys history.disabled and history.dir are deprecated; use persistent history.mode = { dir = "..." } instead."#.to_string()),
        (false, true, None) => Some(r#"Config key history.dir is deprecated; use history.mode = { dir = "..." } instead."#.to_string()),
        (false, false, Some(true)) => Some(r#"Config key history.disabled is deprecated; use history.mode = "volatile" instead."#.to_string()),
        (false, false, Some(false)) => Some(r#"Config key history.disabled is deprecated; use history.mode = "persistent" instead."#.to_string()),
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
#[path = "tests/mod.rs"]
mod tests;
