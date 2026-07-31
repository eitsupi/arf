//! Core R source resolution.

use super::overrides::{OverrideResolution, setup_r_via_overrides};
use super::rig::{ResolvedRSource, resolve_r_home_from_path, setup_r_via_rig};
use crate::config::{RSource, RSourceMode, RSourceOverride, RSourceOverrideInfo, RSourceStatus};
use crate::external;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Set up R based on r_source configuration.
///
/// CLI options override config in this order:
/// 1. `cli_r_home` - explicit R_HOME path
/// 2. `cli_version` - rig version specification
/// 3. Experimental directory-level R source overrides
/// 4. Config `r_source` setting
///
/// The state of override resolution for reporting to users and headless clients.
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
    /// A CLI R source took precedence over overrides.
    ShadowedByCli,
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
    pub(crate) code: &'static str,
    pub(crate) severity: &'static str,
    pub(crate) message: String,
    pub(crate) path: Option<PathBuf>,
}

pub(super) fn warning(
    code: &'static str,
    message: impl Into<String>,
    path: Option<PathBuf>,
) -> RSourceDiagnostic {
    RSourceDiagnostic {
        code,
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
    pub(crate) provider: Option<String>,
    pub(crate) file: Option<std::path::PathBuf>,
    pub(crate) key: Option<String>,
    pub(crate) requested_version: Option<String>,
    pub(crate) resolved_version: Option<String>,
    pub(crate) diagnostics: Vec<RSourceDiagnostic>,
    pub(crate) override_state: RSourceOverrideState,
}

impl RSourceResolutionReport {
    pub(super) fn from_status(
        status: RSourceStatus,
        r_home: Option<PathBuf>,
        override_state: RSourceOverrideState,
    ) -> Self {
        Self {
            status,
            r_home,
            provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state,
        }
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
            provider: Some(info.provider),
            file: info.file,
            key: info.key,
            requested_version: Some(info.requested_version),
            resolved_version: Some(info.resolved_version),
            diagnostics,
            override_state: RSourceOverrideState::Applied,
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
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!("{block}");
        log::warn!("{block}");
    }

    pub(crate) fn override_info(&self) -> Option<RSourceOverrideInfo> {
        Some(RSourceOverrideInfo {
            provider: self.provider.clone()?,
            file: self.file.clone(),
            key: self.key.clone(),
            requested_version: self.requested_version.clone()?,
            resolved_version: self.resolved_version.clone()?,
        })
    }
}

/// Resolve R and return a report without changing the process environment.
pub(crate) fn resolve_r_source(
    r_source: &RSource,
    r_source_overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    cli_r_home: Option<&std::path::Path>,
    cli_version: Option<&str>,
    no_r_source_overrides: bool,
) -> Result<RSourceResolutionReport> {
    // CLI --r-home overrides everything. An explicit disable still controls
    // the reported override state without changing the selected R source.
    if let Some(path) = cli_r_home {
        if !path.exists() {
            anyhow::bail!(
                "R_HOME path does not exist: {}\n\
                 Check your --r-home argument.",
                path.display()
            );
        }
        // Resolve R_HOME: if path looks like an installation prefix (has bin/R),
        // run `bin/R RHOME` to get the actual R_HOME directory
        let r_home = resolve_r_home_from_path(path)?;
        log::info!("Using R from --r-home: {}", r_home.display());
        return Ok(RSourceResolutionReport::from_status(
            RSourceStatus::ExplicitPath {
                path: r_home.clone(),
            },
            Some(r_home),
            if no_r_source_overrides {
                RSourceOverrideState::Disabled
            } else {
                RSourceOverrideState::ShadowedByCli
            },
        ));
    }

    // CLI --with-r-version overrides config (uses rig)
    if let Some(version) = cli_version {
        return setup_r_via_rig(version).map(|resolved| {
            RSourceResolutionReport::from_status(
                resolved.status,
                resolved.r_home,
                if no_r_source_overrides {
                    RSourceOverrideState::Disabled
                } else {
                    RSourceOverrideState::ShadowedByCli
                },
            )
        });
    }

    if no_r_source_overrides {
        return setup_r_fallback(r_source, RSourceOverrideState::Disabled);
    }

    if let Some(result) = setup_r_via_overrides(r_source_overrides, base_dir) {
        match result {
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
    Ok(())
}

/// Set up R and return a resolution report for display and feature gating.
pub(crate) fn setup_r(
    r_source: &RSource,
    r_source_overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    cli_r_home: Option<&std::path::Path>,
    cli_version: Option<&str>,
    no_r_source_overrides: bool,
) -> Result<RSourceResolutionReport> {
    let resolution = resolve_r_source(
        r_source,
        r_source_overrides,
        base_dir,
        cli_r_home,
        cli_version,
        no_r_source_overrides,
    )?;
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
        RSource::Mode(RSourceMode::Auto) => {
            // Auto mode: try rig if available, otherwise use PATH
            if rig_available().is_ok() {
                match resolve_version("default") {
                    Ok(resolved) => {
                        log::info!("Using rig default R version: {}", resolved.version);
                        ResolvedRSource {
                            status: RSourceStatus::Rig {
                                version: resolved.version,
                                override_info: None,
                            },
                            r_home: Some(PathBuf::from(resolved.r_home)),
                        }
                    }
                    Err(e) => {
                        log::debug!("Could not get rig default version: {}", e);
                        log::info!("Using R from PATH");
                        ResolvedRSource {
                            // Do not discover or assign R_HOME here. R will discover it
                            // during initialization, preserving PATH-mode startup behavior.
                            status: RSourceStatus::Path,
                            r_home: None,
                        }
                    }
                }
            } else {
                log::info!("Using R from PATH (rig not available)");
                ResolvedRSource {
                    // Do not discover or assign R_HOME here. R will discover it
                    // during initialization, preserving PATH-mode startup behavior.
                    status: RSourceStatus::Path,
                    r_home: None,
                }
            }
        }
        RSource::Mode(RSourceMode::Rig) => {
            // Rig mode: require rig
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
            match resolve_version("default") {
                Ok(resolved) => {
                    log::info!("Using rig default R version: {}", resolved.version);
                    ResolvedRSource {
                        status: RSourceStatus::Rig {
                            version: resolved.version,
                            override_info: None,
                        },
                        r_home: Some(PathBuf::from(resolved.r_home)),
                    }
                }
                Err(e) => {
                    anyhow::bail!("Failed to get rig default R version: {}", e);
                }
            }
        }
        RSource::Path { path } => {
            // Explicit path mode
            if !path.exists() {
                anyhow::bail!(
                    "R_HOME path does not exist: {}\n\
                     Check your r_source configuration.",
                    path.display()
                );
            }
            log::info!("Using R from explicit path: {}", path.display());
            ResolvedRSource {
                status: RSourceStatus::ExplicitPath { path: path.clone() },
                r_home: Some(path.clone()),
            }
        }
    };

    Ok(RSourceResolutionReport::from_status(
        resolved.status,
        resolved.r_home,
        override_state,
    ))
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
        Err(error) => report.diagnostics.push(warning(
            "r_discovery.failed",
            format!("Warning: Failed to determine R_HOME from PATH: {error}"),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_rig_mode_reports_command_failure_without_install_guidance() {
        let result = setup_r_fallback_with(
            &RSource::Mode(RSourceMode::Rig),
            RSourceOverrideState::NotConfigured,
            || {
                Err(external::rig::RigError::CommandFailed(
                    "permission denied".to_string(),
                ))
            },
            |_| panic!("rig's default version should not be resolved"),
        );

        let error = result.expect_err("rig command failure should reject explicit rig mode");
        let message = error.to_string();
        assert!(message.contains("permission denied"));
        assert!(!message.contains("Install rig"));
    }
}
