//! R_HOME discovery without initializing R.

use crate::app::config_load::{load_config_collecting_warnings, load_config_or_warn};
use crate::app::headless::HeadlessRSourceOverride;
use crate::app::setup::resolve_r_source;
#[cfg(test)]
use crate::config::Config;
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
struct RHomeInfo {
    r_home: String,
    source: String,
    r_source_override: HeadlessRSourceOverride,
    warnings: Vec<String>,
}

/// Resolve and print the R_HOME that startup would use, without initializing R.
pub(crate) fn run_r_home(
    config_path: Option<&Path>,
    r_home: Option<&Path>,
    r_version: Option<&str>,
    no_r_source_overrides: bool,
    json: bool,
) -> Result<()> {
    let mut warnings = Vec::new();
    let config = if json {
        load_config_collecting_warnings(config_path.map(Path::to_path_buf).as_ref(), &mut warnings)
    } else {
        load_config_or_warn(config_path.map(Path::to_path_buf).as_ref())
    };

    let resolution = resolve_r_source(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        None,
        r_home,
        r_version,
        no_r_source_overrides,
    )?;

    if json {
        warnings.extend(resolution.diagnostics.iter().cloned());
        let info = RHomeInfo {
            r_home: resolution.r_home.display().to_string(),
            source: resolution.status.display(),
            r_source_override: HeadlessRSourceOverride::from_report(&resolution),
            warnings,
        };
        println!("{}", serde_json::to_string(&info)?);
    } else {
        resolution.emit_diagnostics();
        println!("{}", resolution.r_home.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_uses_the_headless_override_shape() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::default();
        let resolution = resolve_r_source(
            &config.startup.r_source,
            &config.experimental.r_source_overrides,
            None,
            Some(temp.path()),
            None,
            false,
        )
        .unwrap();
        let value = serde_json::to_value(RHomeInfo {
            r_home: resolution.r_home.display().to_string(),
            source: resolution.status.display(),
            r_source_override: HeadlessRSourceOverride::from_report(&resolution),
            warnings: Vec::new(),
        })
        .unwrap();

        assert_eq!(value["r_home"], temp.path().display().to_string());
        assert_eq!(value["source"], format!("path ({})", temp.path().display()));
        assert_eq!(value["r_source_override"]["state"], "shadowed_by_cli");
        assert!(value["warnings"].as_array().unwrap().is_empty());
    }
}
