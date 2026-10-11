//! Carry an applied environment target across the Unix loader exec, never across :restart.

use super::overrides::configured_provider;
use super::pixi::resolve_strict as resolve_pixi;
use super::r_source::{
    RSourceDiagnostic, RSourceOverrideState, RSourceResolutionReport, RSourceSelectionRoute,
};
use super::rig_project::{RigProjectFailure, resolve_strict as resolve_rig_project};
use crate::app::explicit_r_source::{ExplicitRSource, ExplicitRSourceRequest, RProvider};
use crate::app::startup_env::decode_os_string;
#[cfg(any(unix, test))]
use crate::app::startup_env::encode_os_string;
use crate::config::{
    PixiRSourceInfo, RSourceOverride, RSourceStatus, RigProjectRSourceInfo, VerifiedRTarget,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

const CARRIER: &str = "_ARF_INTERNAL_R_SOURCE_LOADER_TARGET";
static RESTORED: OnceLock<Option<OsString>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
struct LoaderCarrier {
    route: CarrierRoute,
    provider: ProviderCarrier,
    target: TargetCarrier,
    diagnostics: Vec<DiagnosticCarrier>,
}

/// Provider identity; the verified R target is carried separately.
#[derive(Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case")]
enum ProviderCarrier {
    Pixi {
        manifest: String,
        environment: String,
        prefix: String,
        version: Option<String>,
    },
    RigProject {
        root: String,
        manifest: String,
        installation_name: String,
    },
}

#[derive(Serialize, Deserialize)]
struct TargetCarrier {
    binary: String,
    home: String,
    library: String,
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
    let target = report.status.verified_target()?;
    let provider = match &report.status {
        RSourceStatus::Pixi { info } => ProviderCarrier::Pixi {
            manifest: encode_os_string(info.manifest.as_os_str()),
            environment: info.environment.clone(),
            prefix: encode_os_string(info.prefix.as_os_str()),
            version: info.version.clone(),
        },
        RSourceStatus::RigProject { info } => ProviderCarrier::RigProject {
            root: encode_os_string(info.root.as_os_str()),
            manifest: encode_os_string(info.manifest.as_os_str()),
            installation_name: info.installation_name.clone(),
        },
        _ => return None,
    };
    let carrier = LoaderCarrier {
        route,
        provider,
        target: TargetCarrier {
            binary: encode_os_string(target.binary.as_os_str()),
            home: encode_os_string(target.home.as_os_str()),
            library: encode_os_string(target.library.as_os_str()),
        },
        diagnostics,
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

fn deserialize(carrier: &str) -> Result<(RSourceStatus, CarrierRoute, Vec<RSourceDiagnostic>)> {
    let LoaderCarrier {
        route,
        provider,
        target,
        diagnostics,
    } = serde_json::from_str(carrier).context("Invalid R source loader target")?;
    let path = |value: &str| -> Result<PathBuf> {
        decode_os_string(value)
            .map(PathBuf::from)
            .map_err(|_| anyhow::anyhow!("Invalid path in R source loader target"))
    };
    let target = VerifiedRTarget {
        binary: path(&target.binary)?,
        home: path(&target.home)?,
        library: path(&target.library)?,
    };
    let status = match provider {
        ProviderCarrier::Pixi {
            manifest,
            environment,
            prefix,
            version,
        } => RSourceStatus::Pixi {
            info: PixiRSourceInfo {
                manifest: path(&manifest)?,
                environment,
                prefix: path(&prefix)?,
                target,
                version,
            },
        },
        ProviderCarrier::RigProject {
            root,
            manifest,
            installation_name,
        } => RSourceStatus::RigProject {
            info: RigProjectRSourceInfo {
                root: path(&root)?,
                manifest: path(&manifest)?,
                target,
                installation_name,
            },
        },
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
    Ok((status, route, diagnostics))
}

pub(crate) fn restored_target(
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
    r_source_overrides: &[RSourceOverride],
) -> Option<Result<RSourceResolutionReport>> {
    let carrier = RESTORED.get()?.as_ref()?;
    Some((|| {
        let (status, route, diagnostics) = deserialize(
            carrier
                .to_str()
                .context("Invalid R source loader target encoding")?,
        )?;
        let provider = RProvider::of_status(&status)
            .context("The R source loader target has no environment provider")?;
        let (selection_route, override_state) = restore_route(
            route,
            provider,
            explicit_request,
            no_r_source_overrides,
            r_source_overrides,
        )?;
        match &status {
            RSourceStatus::Pixi { info } => validate_pixi(info, resolve_pixi(None))?,
            RSourceStatus::RigProject { info } => {
                validate_rig_project(info, resolve_rig_project(None))?
            }
            _ => unreachable!("the carrier only restores environment providers"),
        }
        Ok(attach_explicit_request(
            RSourceResolutionReport::from_environment(
                status,
                diagnostics,
                selection_route,
                override_state,
            ),
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
            let configured = r_source_overrides
                .iter()
                .any(|source| configured_provider(source) == Some(provider));
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
    // The optional package version is metadata, not part of the target identity.
    anyhow::ensure!(
        previous.manifest == current.manifest
            && previous.environment == current.environment
            && previous.prefix == current.prefix
            && previous.target == current.target,
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
        previous == &current,
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
            target: crate::config::VerifiedRTarget {
                binary: "/prefix/bin/R".into(),
                home: "/prefix/lib/R".into(),
                library: "/prefix/lib/R/lib/libR.so".into(),
            },
            version: None,
        };
        let rig_project = RigProjectRSourceInfo {
            root: "/workspace".into(),
            manifest: "/workspace/rproj.toml".into(),
            target: crate::config::VerifiedRTarget {
                binary: "/workspace/.rvenv/bin/R".into(),
                home: "/rig/R-4.6".into(),
                library: "/rig/R-4.6/lib/libR.so".into(),
            },
            installation_name: "release".into(),
        };
        for status in [
            RSourceStatus::Pixi { info: pixi },
            RSourceStatus::RigProject { info: rig_project },
        ] {
            let report = RSourceResolutionReport {
                status: status.clone(),
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
            let (restored, route, diagnostics) = deserialize(&encoded).unwrap();
            assert_eq!(route, CarrierRoute::Override);
            assert_eq!(diagnostics[0].path, warning.path);
            match (&status, restored) {
                (RSourceStatus::Pixi { info: expected }, RSourceStatus::Pixi { info: actual }) => {
                    assert_eq!(&actual, expected)
                }
                (
                    RSourceStatus::RigProject { info: expected },
                    RSourceStatus::RigProject { info: actual },
                ) => assert_eq!(&actual, expected),
                _ => panic!("carrier provider changed"),
            }
        }
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
            target: crate::config::VerifiedRTarget {
                binary: "/prefix/bin/R".into(),
                home: "/prefix/lib/R".into(),
                library: "/prefix/lib/R/lib/libR.so".into(),
            },
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
                "binary" => changed.target.binary.push("-changed"),
                "home" => changed.target.home.push("-changed"),
                _ => changed.target.library.push("-changed"),
            }
            assert!(validate_pixi(&pixi, Ok(changed)).is_err(), "Pixi {field}");
        }

        let rig_project = RigProjectRSourceInfo {
            root: "/workspace".into(),
            manifest: "/workspace/rproj.toml".into(),
            target: crate::config::VerifiedRTarget {
                binary: "/workspace/.rvenv/bin/R".into(),
                home: "/rig/R-4.6".into(),
                library: "/rig/R-4.6/lib/libR.so".into(),
            },
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
                "binary" => changed.target.binary.push("-changed"),
                "home" => changed.target.home.push("-changed"),
                "library" => changed.target.library.push("-changed"),
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
