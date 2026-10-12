//! Core R source resolution.

use super::overrides::{OverrideResolution, setup_r_via_overrides};
use super::pixi::resolve_strict;
use super::rig::{ResolvedRSource, resolve_r_home_from_path, setup_r_via_rig};
use super::rig_project::resolve_strict as resolve_rig_project;
use crate::app::explicit_r_source::{ExplicitRSource, ExplicitRSourceRequest, RProvider};
use crate::config::{
    RSource, RSourceMode, RSourceOverride, RSourceOverrideInfo, RSourceStatus, mask_home_path,
};
use crate::external;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Outcome of evaluating configured R source overrides, retained for reports.
/// An explicit CLI or environment request is tracked separately as the selection route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RSourceOverrideState {
    /// An override selected the active R installation.
    Applied,
    /// No R source override providers are configured.
    NotConfigured,
    /// No configured provider matched a file in the current directory.
    NoMatch,
    /// At least one provider was evaluated but no override could be applied.
    Failed,
    /// Override resolution was disabled by the CLI.
    Disabled,
    /// An explicit CLI or environment R source took precedence over overrides;
    /// the legacy JSON state name remains `shadowed_by_cli`.
    ShadowedByCli,
}

/// The resolution route that selected the active R target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RSourceSelectionRoute {
    Explicit,
    Override,
    Startup,
}

impl RSourceOverrideState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::NotConfigured => "not_configured",
            Self::NoMatch => "no_match",
            Self::Failed => "failed",
            Self::Disabled => "disabled",
            Self::ShadowedByCli => "shadowed_by_cli",
        }
    }
}

/// A warning produced while resolving an R source.
#[derive(Debug, Clone)]
pub(crate) struct RSourceDiagnostic {
    pub(crate) code: String,
    pub(crate) severity: &'static str,
    pub(crate) message: String,
    pub(crate) path: Option<PathBuf>,
}

pub(super) fn warning(
    code: impl Into<String>,
    message: impl Into<String>,
    path: Option<PathBuf>,
) -> RSourceDiagnostic {
    RSourceDiagnostic {
        code: code.into(),
        severity: "warning",
        message: message.into(),
        path,
    }
}

/// The result of resolving the configured R source and any directory override.
#[derive(Debug, Clone)]
pub(crate) struct RSourceResolutionReport {
    pub(crate) status: RSourceStatus,
    pub(crate) r_home: Option<PathBuf>,
    pub(crate) selection_route: RSourceSelectionRoute,
    pub(crate) override_provider: Option<String>,
    pub(crate) file: Option<std::path::PathBuf>,
    pub(crate) key: Option<String>,
    pub(crate) requested_version: Option<String>,
    pub(crate) resolved_version: Option<String>,
    pub(crate) diagnostics: Vec<RSourceDiagnostic>,
    pub(crate) override_state: RSourceOverrideState,
    pub(crate) explicit_request: Option<ExplicitRSourceRequest>,
}

impl RSourceResolutionReport {
    /// Build a report for R selected by an environment provider such as Pixi.
    pub(super) fn from_environment(
        status: RSourceStatus,
        diagnostics: Vec<RSourceDiagnostic>,
        selection_route: RSourceSelectionRoute,
        override_state: RSourceOverrideState,
    ) -> Self {
        let provider =
            RProvider::of_status(&status).expect("an environment provider selected the status");
        let is_override = selection_route == RSourceSelectionRoute::Override;
        debug_assert_eq!(is_override, override_state == RSourceOverrideState::Applied);
        let r_home = status.verified_target().map(|target| target.home.clone());
        let resolved_version = match &status {
            RSourceStatus::Pixi { info } => info.version.clone(),
            _ => None,
        };
        let mut report =
            Self::from_status_with_route(status, r_home, override_state, selection_route);
        report.override_provider = is_override.then(|| provider.as_str().to_owned());
        report.resolved_version = resolved_version;
        report.diagnostics = diagnostics;
        report
    }

    pub(super) fn from_status(
        status: RSourceStatus,
        r_home: Option<PathBuf>,
        override_state: RSourceOverrideState,
    ) -> Self {
        Self::from_status_with_route(
            status,
            r_home,
            override_state,
            RSourceSelectionRoute::Startup,
        )
    }

    pub(super) fn from_explicit_status(
        status: RSourceStatus,
        r_home: Option<PathBuf>,
        override_state: RSourceOverrideState,
    ) -> Self {
        Self::from_status_with_route(
            status,
            r_home,
            override_state,
            RSourceSelectionRoute::Explicit,
        )
    }

    fn from_status_with_route(
        status: RSourceStatus,
        r_home: Option<PathBuf>,
        override_state: RSourceOverrideState,
        selection_route: RSourceSelectionRoute,
    ) -> Self {
        Self {
            status,
            r_home,
            selection_route,
            override_provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state,
            explicit_request: None,
        }
    }

    pub(crate) fn with_explicit_request(mut self, request: &ExplicitRSourceRequest) -> Self {
        self.explicit_request = Some(request.clone());
        self
    }

    pub(super) fn applied(
        status: RSourceStatus,
        r_home: PathBuf,
        info: RSourceOverrideInfo,
        diagnostics: Vec<RSourceDiagnostic>,
    ) -> Self {
        Self {
            status,
            r_home: Some(r_home),
            selection_route: RSourceSelectionRoute::Override,
            override_provider: Some(info.provider),
            file: Some(info.file),
            key: info.key,
            requested_version: Some(info.requested_version),
            resolved_version: Some(info.resolved_version),
            diagnostics,
            override_state: RSourceOverrideState::Applied,
            explicit_request: None,
        }
    }

    /// Emit all override diagnostics as one warning block for normal startup.
    pub(crate) fn emit_diagnostics(&self) {
        if self.diagnostics.is_empty() {
            return;
        }
        let block = self
            .diagnostics
            .iter()
            .map(display_message)
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!("{block}");
    }

    pub(crate) fn override_info(&self) -> Option<RSourceOverrideInfo> {
        Some(RSourceOverrideInfo {
            provider: self.override_provider.clone()?,
            file: self.file.clone()?,
            key: self.key.clone(),
            requested_version: self.requested_version.clone()?,
            resolved_version: self.resolved_version.clone()?,
        })
    }
}

fn display_message(diagnostic: &RSourceDiagnostic) -> String {
    let Some(path) = diagnostic.path.as_ref() else {
        return diagnostic.message.clone();
    };

    let raw_path = path.display().to_string();
    diagnostic.message.replace(&raw_path, &mask_home_path(path))
}

fn finalize_fallback_diagnostic(report: &mut RSourceResolutionReport) {
    if let Some(diagnostic) = report.diagnostics.last_mut()
        && diagnostic.code == "r_source_override.fallback"
    {
        diagnostic.message = format!(
            "Warning: All R source overrides failed.\n         Using startup.r_source: {}.",
            report.status.display()
        );
        if let RSourceStatus::ExplicitPath { path } = &report.status {
            // Keep real paths in JSON; human rendering masks the associated path.
            diagnostic.path = Some(path.clone());
        }
    }
}

/// Resolve R and return a report without changing the process environment.
pub(crate) fn resolve_r_source(
    r_source: &RSource,
    r_source_overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
) -> Result<RSourceResolutionReport> {
    // Explicit selections take precedence over every configured source.
    if let Some(request) = explicit_request {
        return resolve_explicit(request, base_dir, no_r_source_overrides);
    }

    // With no explicit request, disabling overrides skips only the configured
    // provider chain and continues with startup.r_source.
    if no_r_source_overrides {
        return setup_r_fallback(r_source, RSourceOverrideState::Disabled);
    }

    if let Some(result) = setup_r_via_overrides(r_source_overrides, base_dir) {
        match result {
            OverrideResolution::EnvironmentApplied {
                status,
                diagnostics,
            } => {
                return Ok(RSourceResolutionReport::from_environment(
                    *status,
                    diagnostics,
                    RSourceSelectionRoute::Override,
                    RSourceOverrideState::Applied,
                ));
            }
            OverrideResolution::Applied {
                status,
                r_home,
                info,
                diagnostics,
            } => {
                return Ok(RSourceResolutionReport::applied(
                    *status,
                    r_home,
                    info,
                    diagnostics,
                ));
            }
            OverrideResolution::Fallback { diagnostics } => {
                let mut report = setup_r_fallback(r_source, RSourceOverrideState::Failed)?;
                report.diagnostics = diagnostics;
                finalize_fallback_diagnostic(&mut report);
                return Ok(report);
            }
        }
    }

    let override_state = if r_source_overrides.is_empty() {
        RSourceOverrideState::NotConfigured
    } else {
        RSourceOverrideState::NoMatch
    };
    setup_r_fallback(r_source, override_state)
}

/// Resolve an R source requested explicitly by the CLI or environment.
fn resolve_explicit(
    request: &ExplicitRSourceRequest,
    base_dir: Option<&Path>,
    no_r_source_overrides: bool,
) -> Result<RSourceResolutionReport> {
    let override_state = if no_r_source_overrides {
        RSourceOverrideState::Disabled
    } else {
        RSourceOverrideState::ShadowedByCli
    };
    match &request.source {
        ExplicitRSource::Provider(RProvider::Pixi) => {
            let info = resolve_strict(base_dir).map_err(|failure| {
                anyhow::Error::new(failure).context(
                    "The requested Pixi R provider is unavailable; run arf through `pixi run` with an R-enabled environment",
                )
            })?;
            Ok(RSourceResolutionReport::from_environment(
                RSourceStatus::Pixi { info },
                Vec::new(),
                RSourceSelectionRoute::Explicit,
                override_state,
            )
            .with_explicit_request(request))
        }
        ExplicitRSource::Provider(RProvider::RigProject) => {
            let info = resolve_rig_project(base_dir).map_err(|failure| {
                anyhow::Error::new(failure).context(
                    "The requested rig-project R provider is unavailable; use a synchronized rig project in the current directory",
                )
            })?;
            Ok(RSourceResolutionReport::from_environment(
                RSourceStatus::RigProject { info },
                Vec::new(),
                RSourceSelectionRoute::Explicit,
                override_state,
            )
            .with_explicit_request(request))
        }
        ExplicitRSource::Home(path) => {
            if !path.exists() {
                anyhow::bail!(
                    "R_HOME path does not exist: {}\n\
                     Check your --r-home argument.",
                    path.display()
                );
            }
            let r_home = resolve_r_home_from_path(path)?;
            log::info!("Using R from --r-home: {}", r_home.display());
            Ok(RSourceResolutionReport::from_explicit_status(
                RSourceStatus::ExplicitPath {
                    path: r_home.clone(),
                },
                Some(r_home),
                override_state,
            )
            .with_explicit_request(request))
        }
        ExplicitRSource::Version(version) => setup_r_via_rig(version).map(|resolved| {
            RSourceResolutionReport::from_explicit_status(
                resolved.status,
                resolved.r_home,
                override_state,
            )
            .with_explicit_request(request)
        }),
    }
}

/// Apply a resolved R source to the process environment during startup.
pub(crate) fn apply_r_source(resolution: &RSourceResolutionReport) -> Result<()> {
    if matches!(resolution.status, RSourceStatus::Path) {
        return Ok(());
    }

    let r_home = resolution
        .r_home
        .as_ref()
        .context("Resolved R source has no R_HOME")?;
    // SAFETY: This is called single-threaded during startup, before R is initialized.
    unsafe { std::env::set_var("R_HOME", r_home) };
    if let Some(target) = resolution.status.verified_target() {
        // Pixi limits Windows DLL search to its environment prefix; other
        // providers keep R's ordinary search.
        let dependency_prefix = match &resolution.status {
            RSourceStatus::Pixi { info } => Some(info.prefix.clone()),
            _ => None,
        };
        arf_libr::select_r_library(target.library.clone(), dependency_prefix)?;
    }
    Ok(())
}

/// Set up R and return a resolution report for display and feature gating.
pub(crate) fn setup_r(
    r_source: &RSource,
    r_source_overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
) -> Result<RSourceResolutionReport> {
    let resolution = if let Some(carrier) =
        super::reexec::restored_target(explicit_request, no_r_source_overrides, r_source_overrides)
    {
        carrier?
    } else {
        resolve_r_source(
            r_source,
            r_source_overrides,
            base_dir,
            explicit_request,
            no_r_source_overrides,
        )?
    };
    apply_r_source(&resolution)?;
    Ok(resolution)
}

fn setup_r_fallback(
    r_source: &RSource,
    override_state: RSourceOverrideState,
) -> Result<RSourceResolutionReport> {
    setup_r_fallback_with(
        r_source,
        override_state,
        external::rig::rig_available,
        external::rig::resolve_version,
    )
}

pub(super) fn setup_r_fallback_with<FAvailable, FResolve>(
    r_source: &RSource,
    override_state: RSourceOverrideState,
    rig_available: FAvailable,
    resolve_version: FResolve,
) -> Result<RSourceResolutionReport>
where
    FAvailable: Fn() -> std::result::Result<(), external::rig::RigError>,
    FResolve:
        Fn(&str) -> std::result::Result<external::rig::ResolvedVersion, external::rig::RigError>,
{
    let resolved = match r_source {
        RSource::Mode(RSourceMode::Auto) => resolve_auto_mode(&rig_available, &resolve_version),
        RSource::Mode(RSourceMode::Rig) => resolve_rig_mode(&rig_available, &resolve_version)?,
        RSource::Path { path } => resolve_explicit_path(path)?,
    };

    Ok(RSourceResolutionReport::from_status(
        resolved.status,
        resolved.r_home,
        override_state,
    ))
}

/// Report PATH-mode R without discovering or assigning R_HOME. R discovers it
/// during initialization, preserving PATH-mode startup behavior.
fn path_source() -> ResolvedRSource {
    ResolvedRSource {
        status: RSourceStatus::Path,
        r_home: None,
    }
}

/// Auto mode: try rig if available, otherwise use PATH.
fn resolve_auto_mode(
    rig_available: &impl Fn() -> std::result::Result<(), external::rig::RigError>,
    resolve_version: &impl Fn(
        &str,
    ) -> std::result::Result<
        external::rig::ResolvedVersion,
        external::rig::RigError,
    >,
) -> ResolvedRSource {
    if rig_available().is_err() {
        log::info!("Using R from PATH (rig not available)");
        return path_source();
    }
    match resolve_rig_default(resolve_version) {
        Ok(resolved) => resolved,
        Err(e) => {
            log::debug!("Could not get rig default version: {}", e);
            log::info!("Using R from PATH");
            path_source()
        }
    }
}

/// Rig mode: require rig and its default R version.
fn resolve_rig_mode(
    rig_available: &impl Fn() -> std::result::Result<(), external::rig::RigError>,
    resolve_version: &impl Fn(
        &str,
    ) -> std::result::Result<
        external::rig::ResolvedVersion,
        external::rig::RigError,
    >,
) -> Result<ResolvedRSource> {
    match rig_available() {
        Ok(()) => {}
        Err(external::rig::RigError::NotInstalled) => {
            anyhow::bail!(
                r#"r_source = "rig" but rig is not installed.
Install rig from https://github.com/r-lib/rig or use "auto"."#
            );
        }
        Err(external::rig::RigError::CommandFailed(reason)) => {
            anyhow::bail!(
                r#"r_source = "rig" but rig is installed and could not be run: {reason}.
Fix the rig installation or use "auto"."#
            );
        }
        Err(error) => {
            anyhow::bail!(
                r#"r_source = "rig" but rig availability could not be checked: {error}.
Fix the rig installation or use "auto"."#
            );
        }
    }
    resolve_rig_default(resolve_version)
        .map_err(|e| anyhow::anyhow!("Failed to get rig default R version: {}", e))
}

fn resolve_rig_default(
    resolve_version: &impl Fn(
        &str,
    ) -> std::result::Result<
        external::rig::ResolvedVersion,
        external::rig::RigError,
    >,
) -> std::result::Result<ResolvedRSource, external::rig::RigError> {
    let resolved = resolve_version("default")?;
    log::info!("Using rig default R version: {}", resolved.version);
    Ok(ResolvedRSource {
        status: RSourceStatus::Rig {
            version: resolved.version,
            override_info: None,
        },
        r_home: Some(PathBuf::from(resolved.r_home)),
    })
}

/// Explicit path mode.
fn resolve_explicit_path(path: &Path) -> Result<ResolvedRSource> {
    if !path.exists() {
        anyhow::bail!(
            "R_HOME path does not exist: {}\n\
             Check your r_source configuration.",
            path.display()
        );
    }
    log::info!("Using R from explicit path: {}", path.display());
    Ok(ResolvedRSource {
        status: RSourceStatus::ExplicitPath {
            path: path.to_path_buf(),
        },
        r_home: Some(path.to_path_buf()),
    })
}

fn resolve_path_r_home_with<F>(find_r_library: F) -> Result<PathBuf>
where
    F: FnOnce() -> arf_libr::RResult<PathBuf>,
{
    let library_path = find_r_library().context("Failed to discover the R library from PATH")?;
    arf_libr::r_home_from_library_path(&library_path).with_context(|| {
        format!(
            "Failed to derive R_HOME from discovered R library: {}",
            library_path.display()
        )
    })
}

/// Resolve the PATH-mode R_HOME only for callers that need to report it.
pub(crate) fn resolve_path_r_home_for_report(report: &mut RSourceResolutionReport) {
    resolve_path_r_home_for_report_with(report, arf_libr::find_r_library);
}

pub(super) fn resolve_path_r_home_for_report_with<F>(
    report: &mut RSourceResolutionReport,
    find_r_library: F,
) where
    F: FnOnce() -> arf_libr::RResult<PathBuf>,
{
    if !matches!(report.status, RSourceStatus::Path) || report.r_home.is_some() {
        return;
    }

    match resolve_path_r_home_with(find_r_library) {
        Ok(r_home) => report.r_home = Some(r_home),
        Err(error) => {
            // When discovery is disabled the cause already explains itself, so
            // report it alone: the surrounding context describes a PATH search
            // that never ran. Take the payload rather than the `RError`
            // display, whose "not found at:" wording would read as a path.
            let disabled_message = arf_libr::is_r_auto_discovery_disabled()
                .then(
                    || match error.root_cause().downcast_ref::<arf_libr::RError>() {
                        Some(arf_libr::RError::LibraryNotFound(message)) => Some(message.clone()),
                        _ => None,
                    },
                )
                .flatten();
            let message = match disabled_message {
                Some(cause) => format!("Warning: {cause}"),
                None => format!("Warning: Failed to determine R_HOME from PATH: {error}"),
            };
            report
                .diagnostics
                .push(warning("r_discovery.failed", message, None));
        }
    }
}

#[cfg(test)]
mod tests;
