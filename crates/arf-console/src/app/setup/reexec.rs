//! Carry an applied environment target across the Unix loader exec, never across :restart.

use super::pixi::resolve_strict as resolve_pixi;
use super::r_source::{
    RSourceDiagnostic, RSourceOverrideState, RSourceResolutionReport, RSourceSelectionRoute,
};
use super::rig_project::{RigProjectFailure, resolve_strict as resolve_rig_project};
use crate::app::explicit_r_source::{ExplicitRSource, ExplicitRSourceRequest, RProvider};
use crate::app::startup_env::decode_os_string;
#[cfg(any(unix, test))]
use crate::app::startup_env::encode_os_string;
#[cfg(any(unix, test))]
use crate::config::RSourceStatus;
use crate::config::{PixiRSourceInfo, RSourceOverride, RigProjectRSourceInfo};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

const CARRIER: &str = "_ARF_INTERNAL_R_SOURCE_LOADER_TARGET";
static RESTORED: OnceLock<Option<OsString>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case")]
enum TargetCarrier {
    Pixi {
        route: CarrierRoute,
        info: PixiCarrierInfo,
        diagnostics: Vec<DiagnosticCarrier>,
    },
    RigProject {
        route: CarrierRoute,
        info: RigProjectCarrierInfo,
        diagnostics: Vec<DiagnosticCarrier>,
    },
}

#[derive(Serialize, Deserialize)]
struct PixiCarrierInfo {
    manifest: String,
    environment: String,
    prefix: String,
    binary: String,
    home: String,
    library: String,
    version: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct RigProjectCarrierInfo {
    root: String,
    manifest: String,
    binary: String,
    home: String,
    library: String,
    installation_name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
enum CarrierRoute {
    Explicit,
    Override,
}

#[derive(Serialize, Deserialize)]
struct DiagnosticCarrier {
    code: String,
    message: String,
    path: Option<String>,
}

enum RestoredTarget {
    Pixi(PixiRSourceInfo),
    RigProject(RigProjectRSourceInfo),
}

pub(crate) fn capture_loader_target() {
    // SAFETY: Called at process startup before threads/R initialization. Remove
    // the carrier immediately so :restart independently revalidates activation.
    let carrier = unsafe {
        let value = std::env::var_os(CARRIER);
        std::env::remove_var(CARRIER);
        value
    };
    let _ = RESTORED.set(carrier);
}

#[cfg(unix)]
pub(crate) fn forward_loader_target(report: &RSourceResolutionReport) {
    let Some(carrier) = serialize_report(report) else {
        return;
    };
    // SAFETY: Called only immediately before the single-threaded loader exec.
    unsafe { std::env::set_var(CARRIER, carrier) };
}

#[cfg(unix)]
pub(crate) fn clear_loader_target() {
    // SAFETY: Called if the single-threaded loader boundary returns without exec.
    unsafe { std::env::remove_var(CARRIER) };
}

#[cfg(any(unix, test))]
fn serialize_report(report: &RSourceResolutionReport) -> Option<String> {
    let route = carrier_route(report.selection_route)?;
    let diagnostics = report
        .diagnostics
        .iter()
        .map(|diagnostic| DiagnosticCarrier {
            code: diagnostic.code.clone(),
            message: diagnostic.message.clone(),
            path: diagnostic
                .path
                .as_ref()
                .map(|path| encode_os_string(path.as_os_str())),
        })
        .collect();
    let carrier = match &report.status {
        RSourceStatus::Pixi { info } => TargetCarrier::Pixi {
            route,
            info: PixiCarrierInfo {
                manifest: encode_os_string(info.manifest.as_os_str()),
                environment: info.environment.clone(),
                prefix: encode_os_string(info.prefix.as_os_str()),
                binary: encode_os_string(info.binary.as_os_str()),
                home: encode_os_string(info.home.as_os_str()),
                library: encode_os_string(info.library.as_os_str()),
                version: info.version.clone(),
            },
            diagnostics,
        },
        RSourceStatus::RigProject { info } => TargetCarrier::RigProject {
            route,
            info: RigProjectCarrierInfo {
                root: encode_os_string(info.root.as_os_str()),
                manifest: encode_os_string(info.manifest.as_os_str()),
                binary: encode_os_string(info.binary.as_os_str()),
                home: encode_os_string(info.home.as_os_str()),
                library: encode_os_string(info.library.as_os_str()),
                installation_name: info.installation_name.clone(),
            },
            diagnostics,
        },
        _ => return None,
    };
    // Every value is a string, option or sequence, all serializable as JSON.
    Some(serde_json::to_string(&carrier).expect("string-only loader carrier is serializable"))
}

#[cfg(any(unix, test))]
fn carrier_route(route: RSourceSelectionRoute) -> Option<CarrierRoute> {
    match route {
        RSourceSelectionRoute::Explicit => Some(CarrierRoute::Explicit),
        RSourceSelectionRoute::Override => Some(CarrierRoute::Override),
        RSourceSelectionRoute::Startup => None,
    }
}

fn deserialize(carrier: &str) -> Result<(RestoredTarget, CarrierRoute, Vec<RSourceDiagnostic>)> {
    let carrier: TargetCarrier =
        serde_json::from_str(carrier).context("Invalid R source loader target")?;
    let path = |value: &str| -> Result<PathBuf> {
        decode_os_string(value)
            .map(PathBuf::from)
            .map_err(|_| anyhow::anyhow!("Invalid path in R source loader target"))
    };
    let (target, route, diagnostics) = match carrier {
        TargetCarrier::Pixi {
            route,
            info,
            diagnostics,
        } => (
            RestoredTarget::Pixi(PixiRSourceInfo {
                manifest: path(&info.manifest)?,
                environment: info.environment,
                prefix: path(&info.prefix)?,
                binary: path(&info.binary)?,
                home: path(&info.home)?,
                library: path(&info.library)?,
                version: info.version,
            }),
            route,
            diagnostics,
        ),
        TargetCarrier::RigProject {
            route,
            info,
            diagnostics,
        } => (
            RestoredTarget::RigProject(RigProjectRSourceInfo {
                root: path(&info.root)?,
                manifest: path(&info.manifest)?,
                binary: path(&info.binary)?,
                home: path(&info.home)?,
                library: path(&info.library)?,
                installation_name: info.installation_name,
            }),
            route,
            diagnostics,
        ),
    };
    let diagnostics = diagnostics
        .into_iter()
        .map(|diagnostic| {
            Ok(RSourceDiagnostic {
                code: diagnostic.code,
                severity: "warning",
                message: diagnostic.message,
                path: diagnostic.path.as_deref().map(path).transpose()?,
            })
        })
        .collect::<Result<_>>()?;
    Ok((target, route, diagnostics))
}

pub(crate) fn restored_target(
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
    r_source_overrides: &[RSourceOverride],
) -> Option<Result<RSourceResolutionReport>> {
    let carrier = RESTORED.get()?.as_ref()?;
    Some((|| {
        let (target, route, diagnostics) = deserialize(
            carrier
                .to_str()
                .context("Invalid R source loader target encoding")?,
        )?;
        let provider = match target {
            RestoredTarget::Pixi(info) => {
                let (selection_route, override_state) = restore_route(
                    route,
                    RProvider::Pixi,
                    explicit_request,
                    no_r_source_overrides,
                    r_source_overrides,
                )?;
                validate_pixi(&info, resolve_pixi(None))?;
                return Ok(attach_explicit_request(
                    RSourceResolutionReport::from_pixi(
                        info,
                        diagnostics,
                        selection_route,
                        override_state,
                    ),
                    selection_route,
                    explicit_request,
                ));
            }
            RestoredTarget::RigProject(info) => {
                let (selection_route, override_state) = restore_route(
                    route,
                    RProvider::RigProject,
                    explicit_request,
                    no_r_source_overrides,
                    r_source_overrides,
                )?;
                validate_rig_project(&info, resolve_rig_project())?;
                RSourceResolutionReport::from_rig_project(
                    info,
                    diagnostics,
                    selection_route,
                    override_state,
                )
            }
        };
        let selection_route = provider.selection_route;
        Ok(attach_explicit_request(
            provider,
            selection_route,
            explicit_request,
        ))
    })())
}

fn attach_explicit_request(
    report: RSourceResolutionReport,
    route: RSourceSelectionRoute,
    explicit_request: Option<&ExplicitRSourceRequest>,
) -> RSourceResolutionReport {
    if route == RSourceSelectionRoute::Explicit {
        report.with_explicit_request(
            explicit_request.expect("explicit route validates a provider request"),
        )
    } else {
        report
    }
}

fn restore_route(
    route: CarrierRoute,
    provider: RProvider,
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
    r_source_overrides: &[RSourceOverride],
) -> Result<(RSourceSelectionRoute, RSourceOverrideState)> {
    match route {
        CarrierRoute::Explicit => {
            anyhow::ensure!(
                matches!(
                    explicit_request.map(|request| &request.source),
                    Some(ExplicitRSource::Provider(requested)) if *requested == provider
                ),
                "The explicit R provider request changed during loader re-exec"
            );
            Ok((
                RSourceSelectionRoute::Explicit,
                if no_r_source_overrides {
                    RSourceOverrideState::Disabled
                } else {
                    RSourceOverrideState::ShadowedByCli
                },
            ))
        }
        CarrierRoute::Override => {
            let configured = match provider {
                RProvider::Pixi => r_source_overrides
                    .iter()
                    .any(|source| matches!(source, RSourceOverride::Pixi)),
                RProvider::RigProject => r_source_overrides
                    .iter()
                    .any(|source| matches!(source, RSourceOverride::RigProject)),
            };
            anyhow::ensure!(
                explicit_request.is_none() && !no_r_source_overrides && configured,
                "The selected R source was changed during loader re-exec"
            );
            Ok((
                RSourceSelectionRoute::Override,
                RSourceOverrideState::Applied,
            ))
        }
    }
}

fn validate_pixi(
    previous: &PixiRSourceInfo,
    resolution: std::result::Result<PixiRSourceInfo, super::pixi::PixiFailure>,
) -> Result<()> {
    let Ok(current) = resolution else {
        anyhow::bail!("The selected Pixi R target is no longer available after loader re-exec");
    };
    anyhow::ensure!(
        previous.manifest == current.manifest
            && previous.environment == current.environment
            && previous.prefix == current.prefix
            && previous.binary == current.binary
            && previous.home == current.home
            && previous.library == current.library,
        "The selected Pixi R target changed during loader re-exec"
    );
    Ok(())
}

fn validate_rig_project(
    previous: &RigProjectRSourceInfo,
    resolution: std::result::Result<RigProjectRSourceInfo, RigProjectFailure>,
) -> Result<()> {
    let Ok(current) = resolution else {
        anyhow::bail!(
            "The selected rig project R target is no longer available after loader re-exec"
        );
    };
    anyhow::ensure!(
        previous.root == current.root
            && previous.manifest == current.manifest
            && previous.binary == current.binary
            && previous.home == current.home
            && previous.library == current.library
            && previous.installation_name == current.installation_name,
        "The selected rig project R target changed during loader re-exec"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carrier_round_trips_both_environment_targets_and_diagnostics() {
        let warning = super::super::r_source::warning(
            "prior-provider",
            "earlier provider warning",
            Some(PathBuf::from("/workspace/.r-version")),
        );
        let pixi = PixiRSourceInfo {
            manifest: "/workspace/pixi.toml".into(),
            environment: "analysis".into(),
            prefix: "/prefix".into(),
            binary: "/prefix/bin/R".into(),
            home: "/prefix/lib/R".into(),
            library: "/prefix/lib/R/lib/libR.so".into(),
            version: None,
        };
        let rig_project = RigProjectRSourceInfo {
            root: "/workspace".into(),
            manifest: "/workspace/rproj.toml".into(),
            binary: "/workspace/.rvenv/bin/R".into(),
            home: "/rig/R-4.6".into(),
            library: "/rig/R-4.6/lib/libR.so".into(),
            installation_name: "release".into(),
        };
        for (status, expected) in [
            (
                RSourceStatus::Pixi { info: pixi.clone() },
                CarrierProvider::Pixi,
            ),
            (
                RSourceStatus::RigProject {
                    info: rig_project.clone(),
                },
                CarrierProvider::RigProject,
            ),
        ] {
            let report = RSourceResolutionReport {
                status,
                r_home: None,
                selection_route: RSourceSelectionRoute::Override,
                override_provider: None,
                file: None,
                key: None,
                requested_version: None,
                resolved_version: None,
                diagnostics: vec![warning.clone()],
                override_state: RSourceOverrideState::Applied,
                explicit_request: None,
            };
            let encoded = serialize_report(&report).unwrap();
            let (target, route, diagnostics) = deserialize(&encoded).unwrap();
            assert_eq!(route, CarrierRoute::Override);
            assert_eq!(diagnostics[0].path, warning.path);
            match (target, expected) {
                (RestoredTarget::Pixi(actual), CarrierProvider::Pixi) => {
                    assert_eq!(actual, pixi)
                }
                (RestoredTarget::RigProject(actual), CarrierProvider::RigProject) => {
                    assert_eq!(actual, rig_project)
                }
                _ => panic!("carrier provider changed"),
            }
        }
    }

    #[derive(Clone, Copy)]
    enum CarrierProvider {
        Pixi,
        RigProject,
    }

    #[test]
    fn explicit_route_requires_the_same_provider_and_override_route_requires_config() {
        let explicit_pixi = ExplicitRSourceRequest {
            source: ExplicitRSource::Provider(RProvider::Pixi),
            origin: crate::app::explicit_r_source::RSourceOrigin::Cli,
        };
        let explicit_rig = ExplicitRSourceRequest {
            source: ExplicitRSource::Provider(RProvider::RigProject),
            origin: crate::app::explicit_r_source::RSourceOrigin::Environment,
        };
        assert!(
            restore_route(
                CarrierRoute::Explicit,
                RProvider::RigProject,
                Some(&explicit_rig),
                true,
                &[]
            )
            .is_ok()
        );
        assert!(
            restore_route(
                CarrierRoute::Explicit,
                RProvider::RigProject,
                Some(&explicit_pixi),
                false,
                &[]
            )
            .is_err()
        );
        assert!(
            restore_route(
                CarrierRoute::Override,
                RProvider::RigProject,
                None,
                false,
                &[RSourceOverride::RigProject]
            )
            .is_ok()
        );
        assert!(
            restore_route(
                CarrierRoute::Override,
                RProvider::RigProject,
                None,
                false,
                &[RSourceOverride::Pixi]
            )
            .is_err()
        );
    }

    #[test]
    fn reexec_rejects_unavailable_or_changed_selected_targets() {
        let pixi = PixiRSourceInfo {
            manifest: "/workspace/pixi.toml".into(),
            environment: "analysis".into(),
            prefix: "/prefix".into(),
            binary: "/prefix/bin/R".into(),
            home: "/prefix/lib/R".into(),
            library: "/prefix/lib/R/lib/libR.so".into(),
            version: None,
        };
        assert!(
            validate_pixi(
                &pixi,
                Err(super::super::pixi::PixiFailure::NotActive {
                    workspace_manifest: None,
                })
            )
            .is_err()
        );
        for field in [
            "manifest",
            "environment",
            "prefix",
            "binary",
            "home",
            "library",
        ] {
            let mut changed = pixi.clone();
            match field {
                "manifest" => changed.manifest.push("-changed"),
                "environment" => changed.environment.push_str("-changed"),
                "prefix" => changed.prefix.push("-changed"),
                "binary" => changed.binary.push("-changed"),
                "home" => changed.home.push("-changed"),
                _ => changed.library.push("-changed"),
            }
            assert!(validate_pixi(&pixi, Ok(changed)).is_err(), "Pixi {field}");
        }

        let rig_project = RigProjectRSourceInfo {
            root: "/workspace".into(),
            manifest: "/workspace/rproj.toml".into(),
            binary: "/workspace/.rvenv/bin/R".into(),
            home: "/rig/R-4.6".into(),
            library: "/rig/R-4.6/lib/libR.so".into(),
            installation_name: "release".into(),
        };
        assert!(
            validate_rig_project(
                &rig_project,
                Err(RigProjectFailure::ProjectNotFound {
                    path: "/workspace/rproj.toml".into(),
                })
            )
            .is_err()
        );
        for field in [
            "root",
            "manifest",
            "binary",
            "home",
            "library",
            "installation_name",
        ] {
            let mut changed = rig_project.clone();
            match field {
                "root" => changed.root.push("-changed"),
                "manifest" => changed.manifest.push("-changed"),
                "binary" => changed.binary.push("-changed"),
                "home" => changed.home.push("-changed"),
                "library" => changed.library.push("-changed"),
                _ => changed.installation_name.push_str("-changed"),
            }
            assert!(
                validate_rig_project(&rig_project, Ok(changed)).is_err(),
                "rig project {field}"
            );
        }
        assert!(validate_rig_project(&rig_project, Ok(rig_project.clone())).is_ok());
    }
}
