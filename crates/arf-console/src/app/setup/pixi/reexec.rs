//! Carry an applied target across the Unix loader exec, never across :restart.

use super::{PixiFailure, PixiRTarget, resolve_strict};
use crate::app::explicit_r_source::{ExplicitRSource, ExplicitRSourceRequest, RProvider};
use crate::app::setup::r_source::{
    RSourceDiagnostic, RSourceOverrideState, RSourceResolutionReport, RSourceSelectionRoute,
};
use crate::app::startup_env::decode_os_string;
#[cfg(any(unix, test))]
use crate::app::startup_env::encode_os_string;
use crate::config::RSourceOverride;
#[cfg(unix)]
use crate::config::RSourceStatus;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

const CARRIER: &str = "_ARF_INTERNAL_PIXI_LOADER_TARGET";
static RESTORED: OnceLock<Option<OsString>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
struct TargetCarrier {
    route: CarrierRoute,
    manifest: String,
    environment: String,
    prefix: String,
    binary: String,
    home: String,
    library: String,
    version: Option<String>,
    diagnostics: Vec<DiagnosticCarrier>,
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
    let RSourceStatus::Pixi { info } = &report.status else {
        return;
    };
    let carrier = serialize(info, report.selection_route, &report.diagnostics);
    // SAFETY: Called only immediately before the single-threaded loader exec.
    unsafe { std::env::set_var(CARRIER, carrier) };
}

#[cfg(unix)]
pub(crate) fn clear_loader_target() {
    // SAFETY: Called if the single-threaded loader boundary returns without exec.
    unsafe { std::env::remove_var(CARRIER) };
}

#[cfg(any(unix, test))]
fn serialize(
    info: &PixiRTarget,
    route: RSourceSelectionRoute,
    diagnostics: &[RSourceDiagnostic],
) -> String {
    let carrier = TargetCarrier {
        route: match route {
            RSourceSelectionRoute::Explicit => CarrierRoute::Explicit,
            RSourceSelectionRoute::Override => CarrierRoute::Override,
            RSourceSelectionRoute::Startup => {
                unreachable!("Pixi target cannot be selected by startup R source")
            }
        },
        manifest: encode_os_string(info.manifest.as_os_str()),
        environment: info.environment.clone(),
        prefix: encode_os_string(info.prefix.as_os_str()),
        binary: encode_os_string(info.binary.as_os_str()),
        home: encode_os_string(info.home.as_os_str()),
        library: encode_os_string(info.library.as_os_str()),
        version: info.version.clone(),
        diagnostics: diagnostics
            .iter()
            .map(|diagnostic| DiagnosticCarrier {
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
                path: diagnostic
                    .path
                    .as_ref()
                    .map(|path| encode_os_string(path.as_os_str())),
            })
            .collect(),
    };
    // Every value is a string, option or sequence, all serializable as JSON.
    serde_json::to_string(&carrier).expect("string-only loader carrier is serializable")
}

fn deserialize(carrier: &str) -> Result<(PixiRTarget, CarrierRoute, Vec<RSourceDiagnostic>)> {
    let carrier: TargetCarrier =
        serde_json::from_str(carrier).context("Invalid Pixi loader target")?;
    let path = |value: &str| -> Result<PathBuf> {
        decode_os_string(value)
            .map(PathBuf::from)
            .map_err(|_| anyhow::anyhow!("Invalid path in Pixi loader target"))
    };
    let info = PixiRTarget {
        manifest: path(&carrier.manifest)?,
        environment: carrier.environment,
        prefix: path(&carrier.prefix)?,
        binary: path(&carrier.binary)?,
        home: path(&carrier.home)?,
        library: path(&carrier.library)?,
        version: carrier.version,
    };
    let diagnostics = carrier
        .diagnostics
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
    Ok((info, carrier.route, diagnostics))
}

pub(crate) fn restored_target(
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
    r_source_overrides: &[RSourceOverride],
) -> Option<Result<RSourceResolutionReport>> {
    let carrier = RESTORED.get()?.as_ref()?;
    Some((|| {
        let (info, route, diagnostics) = deserialize(
            carrier
                .to_str()
                .context("Invalid Pixi loader target encoding")?,
        )?;
        let (selection_route, override_state) = restore_route(
            route,
            explicit_request,
            no_r_source_overrides,
            r_source_overrides,
        )?;
        validate(&info, resolve_strict(None))?;
        Ok(RSourceResolutionReport::from_pixi(
            info,
            diagnostics,
            selection_route,
            override_state,
        ))
    })())
}

fn restore_route(
    route: CarrierRoute,
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
    r_source_overrides: &[RSourceOverride],
) -> Result<(RSourceSelectionRoute, RSourceOverrideState)> {
    match route {
        CarrierRoute::Explicit => {
            anyhow::ensure!(
                matches!(
                    explicit_request.map(|request| &request.source),
                    Some(ExplicitRSource::Provider(RProvider::Pixi))
                ),
                "The explicit Pixi provider request changed during loader re-exec"
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
            anyhow::ensure!(
                explicit_request.is_none()
                    && !no_r_source_overrides
                    && r_source_overrides
                        .iter()
                        .any(|source| matches!(source, RSourceOverride::Pixi)),
                "The selected Pixi R source was changed during loader re-exec"
            );
            Ok((
                RSourceSelectionRoute::Override,
                RSourceOverrideState::Applied,
            ))
        }
    }
}

fn validate(previous: &PixiRTarget, resolution: Result<PixiRTarget, PixiFailure>) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_carrier_route_must_match_the_reparsed_selection() {
        let explicit = ExplicitRSourceRequest {
            source: ExplicitRSource::Provider(RProvider::Pixi),
            origin: crate::app::explicit_r_source::RSourceOrigin::Environment,
        };
        let pixi_override = [RSourceOverride::Pixi];
        assert_eq!(
            restore_route(CarrierRoute::Explicit, Some(&explicit), true, &[]).unwrap(),
            (
                RSourceSelectionRoute::Explicit,
                RSourceOverrideState::Disabled
            )
        );
        assert!(restore_route(CarrierRoute::Explicit, None, false, &[]).is_err());
        assert_eq!(
            restore_route(CarrierRoute::Override, None, false, &pixi_override).unwrap(),
            (
                RSourceSelectionRoute::Override,
                RSourceOverrideState::Applied
            )
        );
        assert!(
            restore_route(
                CarrierRoute::Override,
                Some(&explicit),
                false,
                &pixi_override
            )
            .is_err()
        );
    }

    #[test]
    fn reexec_carries_native_paths_and_diagnostics_and_rejects_target_changes() {
        let info = PixiRTarget {
            manifest: "/workspace/pixi.toml".into(),
            environment: "analysis".into(),
            prefix: "/prefix".into(),
            binary: "/prefix/bin/R".into(),
            home: "/prefix/lib/R".into(),
            library: "/prefix/lib/R/lib/libR.so".into(),
            version: None,
        };
        let diagnostics = vec![crate::app::setup::r_source::warning(
            "test",
            "earlier provider warning",
            Some(info.manifest.clone()),
        )];
        let (roundtrip, route, warnings) = deserialize(&serialize(
            &info,
            RSourceSelectionRoute::Override,
            &diagnostics,
        ))
        .unwrap();
        assert_eq!(roundtrip, info);
        assert_eq!(route, CarrierRoute::Override);
        assert_eq!(warnings[0].message, diagnostics[0].message);
        assert_eq!(warnings[0].path, diagnostics[0].path);
        assert!(
            validate(
                &info,
                Err(PixiFailure::NotActive {
                    workspace_manifest: None,
                })
            )
            .is_err()
        );
        assert!(
            validate(
                &info,
                Err(PixiFailure::RUnavailable {
                    reason: "R from the active Pixi environment is unavailable",
                    path: None,
                })
            )
            .is_err()
        );
        for field in [
            "binary",
            "home",
            "library",
            "manifest",
            "prefix",
            "environment",
        ] {
            let mut changed = info.clone();
            match field {
                "binary" => changed.binary.push("changed"),
                "home" => changed.home.push("changed"),
                "library" => changed.library.push("changed"),
                "manifest" => changed.manifest.push("changed"),
                "prefix" => changed.prefix.push("changed"),
                _ => changed.environment.push_str("changed"),
            }
            assert!(validate(&info, Ok(changed)).is_err(), "{field}");
        }
        assert!(validate(&info, Ok(info.clone())).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut native = info;
            native.binary = PathBuf::from(OsString::from_vec(b"/prefix/\xff/R".to_vec()));
            assert_eq!(
                deserialize(&serialize(&native, RSourceSelectionRoute::Explicit, &[]))
                    .unwrap()
                    .0,
                native
            );
        }
    }
}
