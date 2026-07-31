//! Machine-facing R source resolution.

use crate::app::config_load::{ConfigLoadWarning, load_config_collecting_diagnostics};
use crate::app::setup::{
    RSourceDiagnostic, RSourceResolutionReport, resolve_path_r_home_for_report, resolve_r_source,
};
use crate::config::{ConfigLoadProvenance, RSourceStatus};
use crate::external::rig::RigError;
use crate::output::{print_json, write_json};
use anyhow::Result;
use serde::Serialize;
use std::io::Write;
use std::path::Path;

/// The origin of the highest-priority R source option.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RSourceOrigin {
    Cli,
    Environment,
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

/// The provenance kind is an open enum. Clients must accept unknown values.
#[derive(Debug, Serialize)]
struct SelectionSource {
    kind: &'static str,
    name: Option<&'static str>,
    path: Option<String>,
    format: Option<&'static str>,
    key: Option<String>,
}

/// The selection condition kind is an open enum. Clients must accept unknown
/// values so arf can add more selection conditions without breaking clients.
#[derive(Debug, Serialize)]
struct SelectedBy {
    kind: &'static str,
    requested_r_home: Option<String>,
    requested_version: Option<String>,
    source: SelectionSource,
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
/// All keys are always present in the JSON output; nullable descriptor fields
/// are emitted as `null` when they do not apply.
#[derive(Debug, Serialize)]
struct ResolveDescriptor {
    schema_version: u32,
    resolved: bool,
    cwd: String,
    target: Option<Target>,
    resolver: Resolver,
    selected_by: SelectedBy,
    provider: Option<String>,
    diagnostics: Vec<Diagnostic>,
}

/// An error in descriptor generation. It is reported as a JSON error on
/// stderr with the protocol error exit code, while successful unresolved
/// queries still return exit code zero.
#[derive(Debug)]
pub(crate) struct ResolveCommandError {
    kind: ResolveCommandErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy)]
enum ResolveCommandErrorKind {
    InvalidInvocation,
    Internal,
}

impl ResolveCommandError {
    fn invalid_invocation(message: impl Into<String>) -> Self {
        Self {
            kind: ResolveCommandErrorKind::InvalidInvocation,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: ResolveCommandErrorKind::Internal,
            message: message.into(),
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn exit_code(&self) -> u8 {
        match self.kind {
            ResolveCommandErrorKind::InvalidInvocation => 2,
            ResolveCommandErrorKind::Internal => 4,
        }
    }

    fn code(&self) -> &'static str {
        match self.kind {
            ResolveCommandErrorKind::InvalidInvocation => "INVALID_PARAMS",
            ResolveCommandErrorKind::Internal => "INTERNAL_ERROR",
        }
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
    error: ResolveErrorDetails<'a>,
}

#[derive(Debug, Serialize)]
struct ResolveErrorDetails<'a> {
    code: &'static str,
    message: &'a str,
    hint: Option<&'static str>,
    data: Option<&'a serde_json::Value>,
}

/// Print a resolve failure as JSON on stderr.
pub(crate) fn print_error(error: &ResolveCommandError) {
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    let _ = write_json(
        &mut stderr,
        &ResolveError {
            error: ResolveErrorDetails {
                code: error.code(),
                message: error.message(),
                hint: None,
                data: None,
            },
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
    let cwd = std::env::current_dir().map_err(|error| {
        ResolveCommandError::internal(format!(
            "Failed to determine the current directory: {error}"
        ))
    })?;
    let mut config_warnings = Vec::new();
    let config_path_buf = config_path.map(Path::to_path_buf);
    let (config, loaded_config) =
        load_config_collecting_diagnostics(config_path_buf.as_ref(), &mut config_warnings);

    let mut resolution = resolve_r_source(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        None,
        r_home,
        r_version,
        no_r_source_overrides,
    )
    .map_err(|error| classify_resolution_error(error, r_home))?;
    resolve_path_r_home_for_report(&mut resolution);

    let descriptor = descriptor(
        &cwd,
        &resolution,
        &config_warnings,
        r_home,
        r_version,
        r_source_origin,
        loaded_config.as_ref(),
    );
    print_json(&descriptor).map_err(|error| ResolveCommandError::internal(format!("{error:#}")))?;
    Ok(())
}

fn classify_resolution_error(error: anyhow::Error, r_home: Option<&Path>) -> ResolveCommandError {
    let invalid_invocation = match error.downcast_ref::<RigError>() {
        Some(
            RigError::NotInstalled
            | RigError::NoVersionsInstalled
            | RigError::NoDefaultVersion
            | RigError::VersionNotFound(_),
        ) => true,
        Some(RigError::CommandFailed(_) | RigError::ParseError(_)) => false,
        None => r_home.is_some_and(|path| !path.exists()),
    };

    if invalid_invocation {
        ResolveCommandError::invalid_invocation(format!("{error:#}"))
    } else {
        ResolveCommandError::internal(format!("{error:#}"))
    }
}

fn descriptor(
    cwd: &Path,
    resolution: &RSourceResolutionReport,
    config_warnings: &[ConfigLoadWarning],
    r_home: Option<&Path>,
    r_version: Option<&str>,
    r_source_origin: Option<RSourceOrigin>,
    loaded_config: Option<&ConfigLoadProvenance>,
) -> ResolveDescriptor {
    let target = resolution.r_home.as_ref().map(|r_home| {
        let r_home = normalize_path(cwd, r_home);
        Target {
            r_home: r_home.display().to_string(),
            r_binary: find_r_binary(&r_home),
            resolved_version: resolved_version(resolution),
        }
    });
    let resolved = target.is_some();

    let selected_by = selected_by(
        cwd,
        resolution,
        r_home,
        r_version,
        r_source_origin,
        loaded_config,
    );
    let diagnostics = config_warnings
        .iter()
        .map(config_diagnostic)
        .chain(resolution.diagnostics.iter().map(resolution_diagnostic))
        .collect();

    ResolveDescriptor {
        schema_version: 1,
        resolved,
        cwd: cwd.display().to_string(),
        target,
        resolver: Resolver {
            name: "arf",
            version: env!("CARGO_PKG_VERSION"),
        },
        selected_by,
        provider: resolved.then(|| provider(&resolution.status)),
        diagnostics,
    }
}

fn selected_by(
    cwd: &Path,
    resolution: &RSourceResolutionReport,
    r_home: Option<&Path>,
    r_version: Option<&str>,
    r_source_origin: Option<RSourceOrigin>,
    loaded_config: Option<&ConfigLoadProvenance>,
) -> SelectedBy {
    if let Some(r_home) = r_home {
        let (source_kind, source_name) = match r_source_origin.unwrap_or(RSourceOrigin::Cli) {
            RSourceOrigin::Cli => ("command_line_argument", "--r-home"),
            RSourceOrigin::Environment => ("environment_variable", "ARF_R_HOME"),
        };
        return SelectedBy {
            kind: "r_home",
            requested_r_home: Some(display_path(cwd, r_home)),
            requested_version: None,
            source: SelectionSource {
                kind: source_kind,
                name: Some(source_name),
                path: None,
                format: None,
                key: None,
            },
        };
    }

    if let Some(r_version) = r_version {
        let (source_kind, source_name) = match r_source_origin.unwrap_or(RSourceOrigin::Cli) {
            RSourceOrigin::Cli => ("command_line_argument", "--with-r-version"),
            RSourceOrigin::Environment => ("environment_variable", "ARF_R_VERSION"),
        };
        return SelectedBy {
            kind: "version_request",
            requested_r_home: None,
            requested_version: Some(r_version.to_owned()),
            source: SelectionSource {
                kind: source_kind,
                name: Some(source_name),
                path: None,
                format: None,
                key: None,
            },
        };
    }

    if resolution.override_state == crate::app::setup::RSourceOverrideState::Applied {
        let format = match resolution.provider.as_deref() {
            Some("version-file") => Some("text"),
            Some("toml-key") => Some("toml"),
            _ => None,
        };
        return SelectedBy {
            kind: "version_request",
            requested_r_home: None,
            requested_version: resolution.requested_version.clone(),
            source: SelectionSource {
                kind: "project_file",
                name: None,
                path: resolution.file.as_ref().map(|path| display_path(cwd, path)),
                format,
                key: resolution.key.clone(),
            },
        };
    }

    let source = if let Some(loaded_config) = loaded_config
        && loaded_config.startup_r_source_present
    {
        SelectionSource {
            kind: "configuration_file",
            name: None,
            path: Some(display_path(cwd, &loaded_config.path)),
            format: Some("toml"),
            key: Some("startup.r_source".to_owned()),
        }
    } else {
        SelectionSource {
            kind: "built_in_default",
            name: None,
            path: None,
            format: None,
            key: None,
        }
    };

    SelectedBy {
        kind: "default",
        requested_r_home: None,
        requested_version: None,
        source,
    }
}

fn display_path(cwd: &Path, path: &Path) -> String {
    normalize_path(cwd, path).display().to_string()
}

fn normalize_path(cwd: &Path, path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
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

fn resolution_diagnostic(diagnostic: &RSourceDiagnostic) -> Diagnostic {
    Diagnostic {
        code: diagnostic.code.to_owned(),
        severity: diagnostic.severity,
        message: diagnostic
            .message
            .strip_prefix("Warning: ")
            .unwrap_or(&diagnostic.message)
            .to_owned(),
        path: diagnostic
            .path
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
            None,
        ))
        .unwrap();

        assert_eq!(value["selected_by"]["kind"], "version_request");
        assert_eq!(
            value["selected_by"]["source"]["kind"],
            "command_line_argument"
        );
        assert_eq!(value["selected_by"]["source"]["name"], "--with-r-version");
        assert_eq!(value["selected_by"]["requested_version"], "4.4.1");
        assert_eq!(value["target"]["resolved_version"], "4.4.1");
    }

    #[test]
    fn resolution_diagnostic_uses_assigned_code_not_message_wording() {
        let first = RSourceDiagnostic {
            code: "r_source_override.value_invalid",
            severity: "warning",
            message: "Warning: original wording".to_owned(),
            path: None,
        };
        let second = RSourceDiagnostic {
            message: "Warning: completely different wording".to_owned(),
            ..first.clone()
        };

        assert_eq!(
            resolution_diagnostic(&first).code,
            "r_source_override.value_invalid"
        );
        assert_eq!(
            resolution_diagnostic(&second).code,
            "r_source_override.value_invalid"
        );
        assert_eq!(
            resolution_diagnostic(&second).message,
            "completely different wording"
        );
    }
}
