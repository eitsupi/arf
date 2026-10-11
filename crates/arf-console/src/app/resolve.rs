//! Machine-facing R source resolution.

use crate::app::config_load::{ConfigLoadWarning, load_config_collecting_diagnostics};
use crate::app::explicit_r_source::{ExplicitRSource, ExplicitRSourceRequest};
use crate::app::r_source_selection::{RSourceMetadata, RSourceSelection};
use crate::app::setup::{
    PixiFailure, RSourceDiagnostic, RSourceResolutionReport, RigProjectFailure,
    resolve_path_r_home_for_report, resolve_r_source,
};
use crate::config::{ConfigLoadProvenance, RSourceStatus};
use crate::external::rig::RigError;
use crate::output::{print_json, write_json};
use anyhow::Result;
use serde::Serialize;
use std::io::Write;
use std::path::Path;

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
    selected_by: crate::app::r_source_selection::SelectedBy,
    provider: Option<String>,
    metadata: Option<RSourceMetadata>,
    diagnostics: Vec<Diagnostic>,
}

/// An error in descriptor generation. It is reported as a JSON error on
/// stderr with the protocol error exit code, while successful unresolved
/// queries still return exit code zero.
#[derive(Debug)]
pub(crate) struct ResolveCommandError {
    kind: ResolveCommandErrorKind,
    message: String,
    data: Option<serde_json::Value>,
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
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: ResolveCommandErrorKind::Internal,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_invocation_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            kind: ResolveCommandErrorKind::InvalidInvocation,
            message: message.into(),
            data: Some(data),
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
                data: error.data.as_ref(),
            },
        },
        false,
    );
    let _ = writeln!(stderr);
}

/// Resolve and print the R target without initializing R.
pub(crate) fn run_resolve(
    config_path: Option<&Path>,
    explicit_request: Option<&ExplicitRSourceRequest>,
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
        explicit_request,
        no_r_source_overrides,
    )
    .map_err(|error| classify_resolution_error(error, explicit_request))?;
    resolve_path_r_home_for_report(&mut resolution);

    let descriptor = descriptor(&cwd, &resolution, &config_warnings, loaded_config.as_ref());
    print_json(&descriptor).map_err(|error| ResolveCommandError::internal(format!("{error:#}")))?;
    Ok(())
}

fn classify_resolution_error(
    error: anyhow::Error,
    explicit_request: Option<&ExplicitRSourceRequest>,
) -> ResolveCommandError {
    if let Some(failure) = error.downcast_ref::<PixiFailure>() {
        let reason_code = match failure {
            PixiFailure::NotActive { .. } => "r_provider.pixi_not_active",
            PixiFailure::EnvironmentInvalid { .. } => "r_provider.pixi_environment_invalid",
            PixiFailure::RUnavailable { .. } => "r_provider.pixi_r_unavailable",
        };
        return ResolveCommandError::invalid_invocation_with_data(
            format!("{error:#}"),
            serde_json::json!({ "reason_code": reason_code }),
        );
    }
    if let Some(failure) = error.downcast_ref::<RigProjectFailure>() {
        let reason_code = match failure {
            RigProjectFailure::ProjectNotFound { .. } => "r_provider.rig_project_not_found",
            RigProjectFailure::StatusUnavailable { .. } => "r_provider.rig_project_unavailable",
            RigProjectFailure::StatusInvalid { .. } => "r_provider.rig_project_status_invalid",
            RigProjectFailure::NotSynced { .. } => "r_provider.rig_project_not_synced",
            RigProjectFailure::RUnavailable { .. } => "r_provider.rig_project_r_unavailable",
        };
        return ResolveCommandError::invalid_invocation_with_data(
            format!("{error:#}"),
            serde_json::json!({ "reason_code": reason_code }),
        );
    }
    let invalid_invocation = match error.downcast_ref::<RigError>() {
        Some(
            RigError::NotInstalled
            | RigError::NoVersionsInstalled
            | RigError::NoDefaultVersion
            | RigError::VersionNotFound(_),
        ) => true,
        Some(RigError::CommandFailed(_) | RigError::ParseError(_)) => false,
        None => matches!(
            explicit_request.map(|request| &request.source),
            Some(ExplicitRSource::Home(path)) if !path.exists()
        ),
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
    loaded_config: Option<&ConfigLoadProvenance>,
) -> ResolveDescriptor {
    let target = resolution.r_home.as_ref().map(|r_home| {
        let r_home = normalize_path(cwd, r_home);
        Target {
            r_home: r_home.display().to_string(),
            r_binary: match &resolution.status {
                RSourceStatus::Pixi { info } => Some(info.binary.display().to_string()),
                RSourceStatus::RigProject { info } => Some(info.binary.display().to_string()),
                _ => find_r_binary(&r_home),
            },
            resolved_version: resolved_version(resolution),
        }
    });
    let resolved = target.is_some();

    let selection = RSourceSelection::from_report(cwd, resolution, loaded_config);
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
        selected_by: selection.selected_by,
        provider: resolved.then_some(selection.provider).flatten(),
        metadata: selection.metadata,
        diagnostics,
    }
}

fn normalize_path(cwd: &Path, path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

fn resolved_version(resolution: &RSourceResolutionReport) -> Option<String> {
    if matches!(resolution.status, RSourceStatus::RigProject { .. }) {
        return None;
    }
    resolution
        .resolved_version
        .clone()
        .or_else(|| match &resolution.status {
            RSourceStatus::Rig { version, .. } => Some(version.clone()),
            RSourceStatus::Path | RSourceStatus::ExplicitPath { .. } => None,
            RSourceStatus::Pixi { info } => info.version.clone(),
            RSourceStatus::RigProject { .. } => None,
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
    #[test]
    fn pixi_descriptor_uses_verified_launcher_and_configured_provider_provenance() {
        let info = crate::config::PixiRSourceInfo {
            manifest: "/workspace/pixi.toml".into(),
            environment: "analysis".into(),
            prefix: "/prefix".into(),
            binary: "/prefix/Scripts/R.exe".into(),
            home: "/prefix/lib/R".into(),
            library: "/prefix/lib/R/bin/x64/R.dll".into(),
            version: None,
        };
        let mut report = super::RSourceResolutionReport {
            status: super::RSourceStatus::Pixi { info: info.clone() },
            r_home: Some(info.home.clone()),
            selection_route: crate::app::setup::RSourceSelectionRoute::Override,
            override_provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: crate::app::setup::RSourceOverrideState::Applied,
            explicit_request: None,
        };
        report.override_provider = Some("pixi".into());
        report.file = Some(info.manifest.clone());
        let value = serde_json::to_value(super::descriptor(
            std::path::Path::new("/"),
            &report,
            &[],
            None,
        ))
        .unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["provider"], "pixi");
        assert_eq!(value["selected_by"]["kind"], "provider_request");
        assert_eq!(value["selected_by"]["requested_provider"], "pixi");
        assert_eq!(value["selected_by"]["source"]["kind"], "configuration_file");
        assert_eq!(value["metadata"]["environment"], "analysis");
        assert_eq!(
            value["metadata"]["manifest"],
            info.manifest.display().to_string()
        );
        assert_eq!(
            value["target"]["r_binary"],
            info.binary.display().to_string()
        );
        assert!(value["target"]["resolved_version"].is_null());
        assert!(value["selected_by"]["requested_version"].is_null());
        assert!(!report.status.rig_enabled());
    }
    use super::*;
    use crate::app::explicit_r_source::RSourceOrigin;
    use crate::app::setup::RSourceOverrideState;

    #[test]
    fn unresolved_report_has_a_null_target() {
        let resolution = RSourceResolutionReport {
            status: RSourceStatus::Path,
            r_home: None,
            selection_route: crate::app::setup::RSourceSelectionRoute::Startup,
            override_provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: RSourceOverrideState::NotConfigured,
            explicit_request: None,
        };
        let value = serde_json::to_value(descriptor(Path::new("/project"), &resolution, &[], None))
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
            selection_route: crate::app::setup::RSourceSelectionRoute::Explicit,
            override_provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: RSourceOverrideState::ShadowedByCli,
            explicit_request: Some(ExplicitRSourceRequest {
                source: ExplicitRSource::Version("4.4.1".to_owned()),
                origin: RSourceOrigin::Cli,
            }),
        };
        let value = serde_json::to_value(descriptor(Path::new("/project"), &resolution, &[], None))
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
            code: "r_source_override.value_invalid".to_owned(),
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
