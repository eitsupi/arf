//! Machine-facing R source resolution.

use crate::app::config_load::{ConfigLoadWarning, load_config_collecting_diagnostics};
use crate::app::setup::{
    RSourceResolutionReport, resolve_path_r_home_for_report, resolve_r_source,
};
use crate::config::RSourceStatus;
use crate::output::{print_json, write_json};
use anyhow::{Context, Result};
use serde::Serialize;
use std::io::Write;
use std::path::Path;

/// The origin of the highest-priority R source option.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RSourceOrigin {
    Cli,
    Environment,
}

impl RSourceOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Environment => "environment",
        }
    }
}

/// A stable diagnostic attached to a resolve descriptor.
///
/// Diagnostic codes are an open enum: clients must not reject unknown codes,
/// and code meanings must not be reused after publication. Messages are for
/// display only and must not be used for machine classification.
#[derive(Debug, Serialize)]
struct Diagnostic {
    code: String,
    severity: &'static str,
    message: String,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct Target {
    r_home: String,
    r_binary: Option<String>,
    resolved_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct OverrideDescriptor {
    #[serde(rename = "type")]
    descriptor_type: String,
    file: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Serialize)]
struct SelectedBy {
    kind: &'static str,
    origin: &'static str,
    #[serde(rename = "override")]
    override_descriptor: Option<OverrideDescriptor>,
    path: Option<String>,
    requested_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct Resolver {
    name: &'static str,
    version: &'static str,
}

/// The public descriptor returned by `arf r resolve`.
///
/// `schema_version` is bumped for removed, renamed, or type-changed fields,
/// changed enum meanings, changed nullability, or changed `resolved`/`target`
/// invariants. Adding optional fields, diagnostic codes, or enum values is
/// additive and does not require a bump. Clients should accept unknown fields
/// and enum values, and reject schema versions above the highest supported
/// version. `resolver.version` identifies arf and is not a schema version.
#[derive(Debug, Serialize)]
struct ResolveDescriptor {
    schema_version: u32,
    resolved: bool,
    cwd: String,
    target: Option<Target>,
    resolver: Resolver,
    selected_by: SelectedBy,
    provider: String,
    diagnostics: Vec<Diagnostic>,
}

/// An error in descriptor generation. It is reported as a JSON error on
/// stderr with the protocol error exit code, while successful unresolved
/// queries still return exit code zero.
#[derive(Debug)]
pub(crate) struct ResolveCommandError {
    message: String,
}

impl ResolveCommandError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ResolveCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ResolveCommandError {}

#[derive(Debug, Serialize)]
struct ResolveError<'a> {
    error: &'a str,
}

/// Print a resolve failure as JSON on stderr.
pub(crate) fn print_error(error: &ResolveCommandError) {
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    let _ = write_json(
        &mut stderr,
        &ResolveError {
            error: error.message(),
        },
        false,
    );
    let _ = writeln!(stderr);
}

/// Resolve and print the R target without initializing R.
pub(crate) fn run_resolve(
    config_path: Option<&Path>,
    r_home: Option<&Path>,
    r_version: Option<&str>,
    r_source_origin: Option<RSourceOrigin>,
    no_r_source_overrides: bool,
) -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to determine the current directory")?;
    let mut config_warnings = Vec::new();
    let config_path_buf = config_path.map(Path::to_path_buf);
    let config = load_config_collecting_diagnostics(config_path_buf.as_ref(), &mut config_warnings);

    let mut resolution = resolve_r_source(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        None,
        r_home,
        r_version,
        no_r_source_overrides,
    )
    .map_err(|error| ResolveCommandError::new(error.to_string()))?;
    resolve_path_r_home_for_report(&mut resolution);

    let descriptor = descriptor(
        &cwd,
        &resolution,
        &config_warnings,
        r_home,
        r_version,
        r_source_origin,
    );
    print_json(&descriptor).map_err(|error| ResolveCommandError::new(error.to_string()))?;
    Ok(())
}

fn descriptor(
    cwd: &Path,
    resolution: &RSourceResolutionReport,
    config_warnings: &[ConfigLoadWarning],
    r_home: Option<&Path>,
    r_version: Option<&str>,
    r_source_origin: Option<RSourceOrigin>,
) -> ResolveDescriptor {
    let target = resolution.r_home.as_ref().map(|r_home| Target {
        r_home: r_home.display().to_string(),
        r_binary: find_r_binary(r_home),
        resolved_version: resolved_version(resolution),
    });

    let selected_by = selected_by(cwd, resolution, r_home, r_version, r_source_origin);
    let diagnostics = config_warnings
        .iter()
        .map(config_diagnostic)
        .chain(
            resolution
                .diagnostics
                .iter()
                .map(|message| resolution_diagnostic(message, resolution)),
        )
        .collect();

    ResolveDescriptor {
        schema_version: 1,
        resolved: target.is_some(),
        cwd: cwd.display().to_string(),
        target,
        resolver: Resolver {
            name: "arf",
            version: env!("CARGO_PKG_VERSION"),
        },
        selected_by,
        provider: provider(&resolution.status),
        diagnostics,
    }
}

fn selected_by(
    cwd: &Path,
    resolution: &RSourceResolutionReport,
    r_home: Option<&Path>,
    r_version: Option<&str>,
    r_source_origin: Option<RSourceOrigin>,
) -> SelectedBy {
    let (kind, origin) = if r_home.is_some() {
        (
            "explicit_r_home",
            r_source_origin.unwrap_or(RSourceOrigin::Cli).as_str(),
        )
    } else if r_version.is_some() {
        (
            "explicit_r_version",
            r_source_origin.unwrap_or(RSourceOrigin::Cli).as_str(),
        )
    } else if resolution.override_state == crate::app::setup::RSourceOverrideState::Applied {
        ("r_source_override", "config")
    } else {
        ("startup_r_source", "config")
    };

    let override_descriptor = if kind == "r_source_override" {
        Some(OverrideDescriptor {
            descriptor_type: resolution
                .provider
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            file: resolution
                .file
                .as_ref()
                .map(|path| path.display().to_string()),
            key: resolution.key.clone(),
        })
    } else {
        None
    };
    let path = resolution.file.as_ref().map(|path| {
        if path.is_absolute() {
            path.clone()
        } else {
            cwd.join(path)
        }
        .display()
        .to_string()
    });

    SelectedBy {
        kind,
        origin,
        override_descriptor,
        path,
        requested_version: if kind == "r_source_override" {
            resolution.requested_version.clone()
        } else {
            None
        },
    }
}

fn provider(status: &RSourceStatus) -> String {
    match status {
        RSourceStatus::Rig { .. } => "rig".to_owned(),
        RSourceStatus::Path => "path".to_owned(),
        RSourceStatus::ExplicitPath { .. } => "explicit_path".to_owned(),
    }
}

fn resolved_version(resolution: &RSourceResolutionReport) -> Option<String> {
    resolution
        .resolved_version
        .clone()
        .or_else(|| match &resolution.status {
            RSourceStatus::Rig { version, .. } => Some(version.clone()),
            RSourceStatus::Path | RSourceStatus::ExplicitPath { .. } => None,
        })
}

fn find_r_binary(r_home: &Path) -> Option<String> {
    #[cfg(windows)]
    let candidates = [
        r_home.join("bin").join("R.exe"),
        r_home.join("bin").join("R.bat"),
    ];
    #[cfg(not(windows))]
    let candidates = [r_home.join("bin").join("R")];

    candidates
        .iter()
        .find(|candidate| candidate.exists())
        .map(|candidate| candidate.display().to_string())
}

fn config_diagnostic(warning: &ConfigLoadWarning) -> Diagnostic {
    Diagnostic {
        code: warning.code.to_owned(),
        severity: "warning",
        message: warning.message.clone(),
        path: Some(warning.path.display().to_string()),
    }
}

fn resolution_diagnostic(message: &str, resolution: &RSourceResolutionReport) -> Diagnostic {
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("rig is not installed") {
        "r_source_override.rig_unavailable"
    } else if lower.contains("not installed") || lower.contains("no installed r version") {
        "r_source_override.version_not_installed"
    } else if lower.contains("unsupported") || lower.contains("not implemented") {
        "r_source_override.provider_unsupported"
    } else if lower.contains("failed to determine r_home")
        || lower.contains("failed to discover")
        || lower.contains("failed to derive r_home")
    {
        "r_discovery.failed"
    } else if lower.contains("falling back") || lower.contains("trying the next") {
        "r_source_override.fallback"
    } else {
        "r_source_override.value_invalid"
    };

    Diagnostic {
        code: code.to_owned(),
        severity: "warning",
        message: message
            .strip_prefix("Warning: ")
            .unwrap_or(message)
            .to_owned(),
        path: resolution
            .file
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::setup::RSourceOverrideState;

    #[test]
    fn unresolved_report_has_a_null_target() {
        let resolution = RSourceResolutionReport {
            status: RSourceStatus::Path,
            r_home: None,
            provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: RSourceOverrideState::NotConfigured,
        };
        let value = serde_json::to_value(descriptor(
            Path::new("/project"),
            &resolution,
            &[],
            None,
            None,
            None,
        ))
        .unwrap();

        assert_eq!(value["resolved"], false);
        assert!(value["target"].is_null());
    }

    #[test]
    fn explicit_version_selection_records_its_origin() {
        let resolution = RSourceResolutionReport {
            status: RSourceStatus::Rig {
                version: "4.4.1".to_owned(),
                override_info: None,
            },
            r_home: Some(Path::new("/opt/R/4.4.1/lib/R").to_owned()),
            provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: RSourceOverrideState::ShadowedByCli,
        };
        let value = serde_json::to_value(descriptor(
            Path::new("/project"),
            &resolution,
            &[],
            None,
            Some("4.4.1"),
            Some(RSourceOrigin::Cli),
        ))
        .unwrap();

        assert_eq!(value["selected_by"]["kind"], "explicit_r_version");
        assert_eq!(value["selected_by"]["origin"], "cli");
        assert_eq!(value["target"]["resolved_version"], "4.4.1");
    }
}
