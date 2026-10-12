//! Directory-level R source overrides.

mod diagnostics;
mod provider;
#[cfg(test)]
mod tests;

use super::pixi::{self, PixiFailure};
use super::r_source::{RSourceDiagnostic, RSourceResolutionReport, RSourceSelectionRoute};
use super::rig::{ResolvedRSource, setup_r_via_selected_rig_version};
use super::rig_project::{RigProjectFailure, resolve_strict as resolve_rig_project};
use crate::config::{
    PixiRSourceInfo, RSourceOverride, RSourceOverrideInfo, RSourceStatus, RigProjectRSourceInfo,
};
use crate::external;
use crate::rversion;
use anyhow::Result;
use diagnostics::{
    empty_version_warning, fallback_warning, installed_versions_unavailable_warning,
    missing_r_home_warning, named_version_unsupported_warning, not_installed_warning,
    pixi_failure_diagnostic, rig_project_failure_diagnostic, rig_unavailable_warning,
    unparsable_version_warning, unsupported_status_warning, version_resolution_failed_warning,
};
pub(super) use provider::configured_provider;
use provider::{ProviderKind, ProviderSpec, ReadOverride, read_override};
use std::path::{Path, PathBuf};

pub(super) enum OverrideResolution {
    /// An environment provider such as Pixi selected a verified R target.
    EnvironmentApplied {
        status: Box<RSourceStatus>,
        diagnostics: Vec<RSourceDiagnostic>,
    },
    Applied {
        status: Box<RSourceStatus>,
        r_home: PathBuf,
        info: RSourceOverrideInfo,
        diagnostics: Vec<RSourceDiagnostic>,
    },
    Fallback {
        diagnostics: Vec<RSourceDiagnostic>,
    },
}

/// The R source chosen by a single provider, before diagnostics are attached.
enum ProviderSelection {
    Environment(RSourceStatus),
    VersionRequest {
        status: RSourceStatus,
        r_home: PathBuf,
        info: RSourceOverrideInfo,
    },
}

impl ProviderSelection {
    fn into_resolution(self, diagnostics: Vec<RSourceDiagnostic>) -> OverrideResolution {
        match self {
            Self::Environment(status) => OverrideResolution::EnvironmentApplied {
                status: Box::new(status),
                diagnostics,
            },
            Self::VersionRequest {
                status,
                r_home,
                info,
            } => OverrideResolution::Applied {
                status: Box::new(status),
                r_home,
                info,
                diagnostics,
            },
        }
    }
}

/// What happened when one configured provider was tried.
enum ProviderOutcome {
    /// The provider has nothing to evaluate here, such as an absent file, and
    /// says nothing about whether overrides were attempted.
    NotApplicable,
    /// The provider was evaluated but did not select R. Any diagnostic has
    /// already been recorded.
    Rejected,
    Selected(Box<ProviderSelection>),
}

impl ProviderOutcome {
    fn selected(selection: ProviderSelection) -> Self {
        Self::Selected(Box::new(selection))
    }
}

/// Try the configured directory-level R source overrides in priority order.
pub(super) fn setup_r_via_overrides(
    overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
) -> Option<OverrideResolution> {
    setup_r_via_overrides_with(
        overrides,
        base_dir,
        external::rig::rig_available,
        external::rig::list_versions,
        setup_r_via_selected_rig_version,
    )
}

fn setup_r_via_overrides_with<FAvailable, FList, FResolve>(
    overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    rig_available: FAvailable,
    list_versions: FList,
    resolve_selected_rig_version: FResolve,
) -> Option<OverrideResolution>
where
    FAvailable: Fn() -> std::result::Result<(), external::rig::RigError>,
    FList: Fn() -> std::result::Result<Vec<external::rig::RigVersion>, external::rig::RigError>,
    FResolve: Fn(&semver::Version, &[external::rig::RigVersion]) -> Result<ResolvedRSource>,
{
    setup_r_via_overrides_with_environment_providers(
        overrides,
        base_dir,
        rig_available,
        list_versions,
        resolve_selected_rig_version,
        pixi::resolve_strict,
        resolve_rig_project,
    )
}

fn setup_r_via_overrides_with_environment_providers<FAvailable, FList, FResolve>(
    overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    rig_available: FAvailable,
    list_versions: FList,
    resolve_selected_rig_version: FResolve,
    resolve_pixi: impl Fn(Option<&Path>) -> std::result::Result<PixiRSourceInfo, PixiFailure>,
    resolve_rig_project: impl Fn(
        Option<&Path>,
    ) -> std::result::Result<RigProjectRSourceInfo, RigProjectFailure>,
) -> Option<OverrideResolution>
where
    FAvailable: Fn() -> std::result::Result<(), external::rig::RigError>,
    FList: Fn() -> std::result::Result<Vec<external::rig::RigVersion>, external::rig::RigError>,
    FResolve: Fn(&semver::Version, &[external::rig::RigVersion]) -> Result<ResolvedRSource>,
{
    let mut diagnostics = Vec::new();
    let mut evaluated_provider = false;
    let mut version_providers =
        VersionProviders::new(rig_available, list_versions, resolve_selected_rig_version);
    let mut base_dir = base_dir.map(Path::to_path_buf);

    for source in overrides {
        let provider = ProviderSpec::from(source);
        let outcome = match provider.kind {
            ProviderKind::Pixi => try_pixi(&resolve_pixi, base_dir.as_deref(), &mut diagnostics),
            ProviderKind::RigProject => {
                try_rig_project(&resolve_rig_project, base_dir.as_deref(), &mut diagnostics)
            }
            ProviderKind::File { .. } => {
                version_providers.try_version_provider(&provider, &mut base_dir, &mut diagnostics)
            }
        };
        match outcome {
            ProviderOutcome::NotApplicable => {}
            ProviderOutcome::Rejected => evaluated_provider = true,
            ProviderOutcome::Selected(selection) => {
                return Some(selection.into_resolution(diagnostics));
            }
        }
    }

    if evaluated_provider {
        diagnostics.push(fallback_warning());
        Some(OverrideResolution::Fallback { diagnostics })
    } else {
        None
    }
}

fn try_pixi(
    resolve_pixi: &impl Fn(Option<&Path>) -> std::result::Result<PixiRSourceInfo, PixiFailure>,
    base_dir: Option<&Path>,
    diagnostics: &mut Vec<RSourceDiagnostic>,
) -> ProviderOutcome {
    match resolve_pixi(base_dir) {
        Err(PixiFailure::NotActive {
            workspace_manifest: None,
        }) => ProviderOutcome::NotApplicable,
        Err(error) => {
            diagnostics.extend(pixi_failure_diagnostic(&error));
            ProviderOutcome::Rejected
        }
        Ok(info) => {
            ProviderOutcome::selected(ProviderSelection::Environment(RSourceStatus::Pixi { info }))
        }
    }
}

fn try_rig_project(
    resolve_rig_project: &impl Fn(
        Option<&Path>,
    ) -> std::result::Result<RigProjectRSourceInfo, RigProjectFailure>,
    base_dir: Option<&Path>,
    diagnostics: &mut Vec<RSourceDiagnostic>,
) -> ProviderOutcome {
    match resolve_rig_project(base_dir) {
        Err(RigProjectFailure::ProjectNotFound { .. }) => ProviderOutcome::NotApplicable,
        Err(error) => {
            diagnostics.push(rig_project_failure_diagnostic(&error));
            ProviderOutcome::Rejected
        }
        Ok(info) => {
            ProviderOutcome::selected(ProviderSelection::Environment(RSourceStatus::RigProject {
                info,
            }))
        }
    }
}

/// Resolves version requests read from files through rig, fetching rig's
/// availability and installed versions at most once per resolution.
struct VersionProviders<FAvailable, FList, FResolve> {
    rig_available: FAvailable,
    list_versions: FList,
    resolve_selected_rig_version: FResolve,
    rig_available_cache: Option<std::result::Result<(), external::rig::RigError>>,
    installed_versions_cache:
        Option<std::result::Result<Vec<external::rig::RigVersion>, external::rig::RigError>>,
}

impl<FAvailable, FList, FResolve> VersionProviders<FAvailable, FList, FResolve>
where
    FAvailable: Fn() -> std::result::Result<(), external::rig::RigError>,
    FList: Fn() -> std::result::Result<Vec<external::rig::RigVersion>, external::rig::RigError>,
    FResolve: Fn(&semver::Version, &[external::rig::RigVersion]) -> Result<ResolvedRSource>,
{
    fn new(
        rig_available: FAvailable,
        list_versions: FList,
        resolve_selected_rig_version: FResolve,
    ) -> Self {
        Self {
            rig_available,
            list_versions,
            resolve_selected_rig_version,
            rig_available_cache: None,
            installed_versions_cache: None,
        }
    }

    /// Read a file provider's version request and resolve it to an installed R.
    fn try_version_provider(
        &mut self,
        provider: &ProviderSpec<'_>,
        base_dir: &mut Option<PathBuf>,
        diagnostics: &mut Vec<RSourceDiagnostic>,
    ) -> ProviderOutcome {
        let version = match read_override(provider, base_dir) {
            ReadOverride::Missing => return ProviderOutcome::NotApplicable,
            ReadOverride::Value(version) => version,
            ReadOverride::Rejected(diagnostic) => {
                diagnostics.push(diagnostic);
                return ProviderOutcome::Rejected;
            }
        };

        match self.resolve_version_request(provider, version.trim()) {
            Ok(selection) => ProviderOutcome::selected(selection),
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                ProviderOutcome::Rejected
            }
        }
    }

    fn resolve_version_request(
        &mut self,
        provider: &ProviderSpec<'_>,
        trimmed_version: &str,
    ) -> std::result::Result<ProviderSelection, RSourceDiagnostic> {
        let spec = match rversion::VersionSpec::parse(trimmed_version) {
            Ok(spec) => spec,
            Err(rversion::VersionSpecParseError::Empty) => {
                return Err(empty_version_warning(provider));
            }
            Err(rversion::VersionSpecParseError::Invalid) => {
                return Err(unparsable_version_warning(provider));
            }
        };

        if let rversion::VersionSpec::Named(name) = &spec {
            return Err(named_version_unsupported_warning(provider, name));
        }

        let rig_availability = self
            .rig_available_cache
            .get_or_insert_with(&self.rig_available);
        if let Err(error) = rig_availability {
            return Err(rig_unavailable_warning(error));
        }

        let installed = match self
            .installed_versions_cache
            .get_or_insert_with(&self.list_versions)
        {
            Ok(versions) => versions,
            Err(error) => return Err(installed_versions_unavailable_warning(provider, error)),
        };
        let installed_versions = installed
            .iter()
            .filter_map(|installed| semver::Version::parse(&installed.version).ok())
            .collect::<Vec<_>>();

        let Some(selected) = rversion::resolve_version(&spec, &installed_versions) else {
            return Err(not_installed_warning(
                provider.name,
                &provider.location(),
                trimmed_version,
                &spec,
                provider.file_path(),
            ));
        };

        match (self.resolve_selected_rig_version)(selected, installed) {
            Ok(ResolvedRSource {
                status: RSourceStatus::Rig { version, .. },
                r_home,
            }) => {
                let Some(file) = provider.file_path() else {
                    unreachable!("only file providers read an R version request");
                };
                let info = RSourceOverrideInfo {
                    provider: provider.name.to_owned(),
                    file,
                    key: provider.key().map(str::to_owned),
                    requested_version: trimmed_version.to_owned(),
                    resolved_version: version.clone(),
                };
                let Some(r_home) = r_home else {
                    return Err(missing_r_home_warning(provider));
                };
                Ok(ProviderSelection::VersionRequest {
                    status: RSourceStatus::Rig {
                        version,
                        override_info: Some(info.clone()),
                    },
                    r_home,
                    info,
                })
            }
            Ok(resolved) => Err(unsupported_status_warning(provider, &resolved)),
            Err(error) => Err(version_resolution_failed_warning(
                provider,
                trimmed_version,
                &error,
            )),
        }
    }
}

/// Format the startup notice for an R source selected by a configured override.
pub(crate) fn override_notice(resolution: &RSourceResolutionReport) -> Option<String> {
    if resolution.selection_route == RSourceSelectionRoute::Override
        && matches!(
            resolution.status,
            RSourceStatus::Pixi { .. } | RSourceStatus::RigProject { .. }
        )
    {
        return Some(format!(
            "# R source override: {}",
            resolution.status.display()
        ));
    }
    resolution
        .override_info()
        .map(|info| format!("# R source override: {}", info.display()))
}
