//! Config loading helpers shared across the REPL, headless, and script paths.

use crate::cli::Cli;
use crate::config::{
    Config, ConfigLoadError, ConfigLoadProvenance, ConfigStatus, config_file_path, load_config,
    load_config_from_path, load_config_from_path_with_provenance, mask_home_path,
};

fn report_history_migration_warning(warning: &str) {
    eprintln!("Warning: {warning}");
    log::warn!("{warning}");
}

/// A config load warning with stable machine-readable classification.
#[derive(Debug)]
pub(crate) struct ConfigLoadWarning {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) path: std::path::PathBuf,
}

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
        Ok(mut config) => {
            if let Some(warning) = config.history_migration_warning.take() {
                report_history_migration_warning(&warning);
            }
            (config, config_path, ConfigStatus::Ok)
        }
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
        Ok(mut config) => {
            if let Some(warning) = config.history_migration_warning.take() {
                report_history_migration_warning(&warning);
            }
            config
        }
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
/// Used by headless JSON output. The resolve command uses
/// `load_config_collecting_diagnostics` to preserve the error classification.
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
        Ok(mut config) => {
            if let Some(warning) = config.history_migration_warning.take() {
                log::warn!("{warning}");
                warnings.push(warning);
            }
            config
        }
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

/// Load config with the same defaults fallback as normal startup, preserving
/// the config error kind for machine-facing diagnostics.
pub(crate) fn load_config_collecting_diagnostics(
    config_path: Option<&std::path::PathBuf>,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> (Config, Option<ConfigLoadProvenance>) {
    let path = config_path.cloned().or_else(config_file_path);
    let result = if let Some(path) = path.as_deref() {
        load_config_from_path_with_provenance(path)
    } else {
        Ok((Config::default(), None))
    };

    match result {
        Ok((mut config, provenance)) => {
            let warning = config.history_migration_warning.take();
            if let (Some(warning), Some(provenance)) = (&warning, provenance.as_ref()) {
                log::warn!("{warning}");
                warnings.push(ConfigLoadWarning {
                    code: "config.history_disabled_deprecated",
                    message: warning.clone(),
                    path: provenance.path.clone(),
                });
            }
            let provenance = provenance.map(|mut provenance| {
                provenance.history_migration_warning = warning;
                provenance
            });
            (config, provenance)
        }
        Err(error) => {
            let (code, path, message) = match error {
                ConfigLoadError::Read { path, source } => {
                    ("config.read_failed", path, source.to_string())
                }
                ConfigLoadError::Parse { path, source } => {
                    ("config.parse_failed", path, source.to_string())
                }
            };
            warnings.push(ConfigLoadWarning {
                code,
                message: format!(
                    "Failed to load config from {}: {}. Using default configuration.",
                    path.display(),
                    message
                ),
                path,
            });
            (Config::default(), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_history_disabled_is_collected_for_headless_output() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("arf.toml");
        std::fs::write(&path, "[history]\ndisabled = true\n").unwrap();
        let mut warnings = Vec::new();
        let config = load_config_collecting_warnings(Some(&path), &mut warnings);

        assert!(matches!(
            config.history.mode,
            crate::config::HistoryMode::Volatile
        ));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("history.mode = \"volatile\""));
    }

    #[test]
    fn deprecated_history_disabled_has_a_stable_diagnostic_code() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("arf.toml");
        std::fs::write(&path, "[history]\ndisabled = false\n").unwrap();
        let mut warnings = Vec::new();
        let (_, provenance) = load_config_collecting_diagnostics(Some(&path), &mut warnings);

        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "config.history_disabled_deprecated");
        assert!(
            provenance
                .unwrap()
                .history_migration_warning
                .unwrap()
                .contains("persistent")
        );
    }
}
