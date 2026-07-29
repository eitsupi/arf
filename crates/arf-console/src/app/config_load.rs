//! Config loading helpers shared across the REPL, headless, and script paths.

use crate::cli::Cli;
use crate::config::{
    Config, ConfigLoadError, ConfigStatus, config_file_path, load_config, load_config_from_path,
    mask_home_path,
};

/// Load configuration with fallback to defaults on error.
///
/// Prints a warning to stderr if the config file has errors.
/// Returns `(config, config_path, config_status)`.
pub(crate) fn load_config_with_fallback(
    cli: &Cli,
) -> (Config, Option<std::path::PathBuf>, ConfigStatus) {
    let (result, config_path) = if let Some(path) = &cli.r_source.config {
        (load_config_from_path(path), Some(path.clone()))
    } else {
        let default_path = config_file_path();
        (load_config(), default_path)
    };

    match result {
        Ok(config) => (config, config_path, ConfigStatus::Ok),
        Err(e) => {
            let (raw_path, masked_path, source_msg, status) = match &e {
                ConfigLoadError::Read { path, source } => (
                    path.display().to_string(),
                    mask_home_path(path),
                    source.to_string(),
                    ConfigStatus::ReadError,
                ),
                ConfigLoadError::Parse { path, source } => (
                    path.display().to_string(),
                    mask_home_path(path),
                    source.to_string(),
                    ConfigStatus::ParseError,
                ),
            };
            eprintln!(
                "Warning: Failed to load config from {}: {}",
                masked_path, source_msg
            );
            eprintln!(
                "         Using default configuration. Run `arf config check` to see details."
            );
            // Log with unmasked path for debugging
            log::warn!("Config load error for {}: {}", raw_path, source_msg);
            (Config::default(), config_path, status)
        }
    }
}

/// Load config with a warning on error, falling back to defaults.
///
/// Used by subcommands (history, script) where config loading is not the
/// primary operation but errors should still be visible.
pub(crate) fn load_config_or_warn(config_path: Option<&std::path::PathBuf>) -> Config {
    let result = if let Some(path) = config_path {
        load_config_from_path(path)
    } else {
        load_config()
    };
    match result {
        Ok(config) => config,
        Err(e) => {
            let (path_display, source_msg) = match &e {
                ConfigLoadError::Read { path, source } => {
                    (mask_home_path(path), source.to_string())
                }
                ConfigLoadError::Parse { path, source } => {
                    (mask_home_path(path), source.to_string())
                }
            };
            eprintln!(
                "Warning: Failed to load config from {}: {}",
                path_display, source_msg
            );
            eprintln!("         Using default configuration.");
            Config::default()
        }
    }
}

/// Load config, collecting warnings into a buffer instead of printing to stderr.
///
/// Used by `--json` mode to include config warnings in the JSON output.
pub(crate) fn load_config_collecting_warnings(
    config_path: Option<&std::path::PathBuf>,
    warnings: &mut Vec<String>,
) -> Config {
    let result = if let Some(path) = config_path {
        load_config_from_path(path)
    } else {
        load_config()
    };
    match result {
        Ok(config) => config,
        Err(e) => {
            let (path_display, source_msg) = match &e {
                ConfigLoadError::Read { path, source } => {
                    (mask_home_path(path), source.to_string())
                }
                ConfigLoadError::Parse { path, source } => {
                    (mask_home_path(path), source.to_string())
                }
            };
            warnings.push(format!(
                "Failed to load config from {path_display}: {source_msg}. Using default configuration."
            ));
            Config::default()
        }
    }
}
