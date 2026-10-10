//! Carry an applied target across the Unix loader exec, never across :restart.

use super::{PixiRTarget, PixiResolution};
use crate::app::setup::r_source::{RSourceDiagnostic, RSourceResolutionReport};
use crate::app::startup_env::decode_os_string;
#[cfg(any(unix, test))]
use crate::app::startup_env::encode_os_string;
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
    manifest: String,
    environment: String,
    prefix: String,
    binary: String,
    home: String,
    library: String,
    version: Option<String>,
    diagnostics: Vec<DiagnosticCarrier>,
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
    let carrier = serialize(info, &report.diagnostics);
    // SAFETY: Called only immediately before the single-threaded loader exec.
    unsafe { std::env::set_var(CARRIER, carrier) };
}

#[cfg(unix)]
pub(crate) fn clear_loader_target() {
    // SAFETY: Called if the single-threaded loader boundary returns without exec.
    unsafe { std::env::remove_var(CARRIER) };
}

#[cfg(any(unix, test))]
fn serialize(info: &PixiRTarget, diagnostics: &[RSourceDiagnostic]) -> String {
    let carrier = TargetCarrier {
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

fn deserialize(carrier: &str) -> Result<(PixiRTarget, Vec<RSourceDiagnostic>)> {
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
    Ok((info, diagnostics))
}

pub(crate) fn restored_target() -> Option<Result<RSourceResolutionReport>> {
    let carrier = RESTORED.get()?.as_ref()?;
    Some((|| {
        let (info, diagnostics) = deserialize(
            carrier
                .to_str()
                .context("Invalid Pixi loader target encoding")?,
        )?;
        validate(&info, super::resolve(None))?;
        Ok(RSourceResolutionReport::from_pixi(info, diagnostics))
    })())
}

fn validate(previous: &PixiRTarget, resolution: PixiResolution) -> Result<()> {
    let PixiResolution::Applied(current) = resolution else {
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
        let (roundtrip, warnings) = deserialize(&serialize(&info, &diagnostics)).unwrap();
        assert_eq!(roundtrip, info);
        assert_eq!(warnings[0].message, diagnostics[0].message);
        assert_eq!(warnings[0].path, diagnostics[0].path);
        assert!(validate(&info, PixiResolution::NoMatch).is_err());
        assert!(validate(&info, PixiResolution::Rejected(diagnostics[0].clone())).is_err());
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
            assert!(
                validate(&info, PixiResolution::Applied(changed)).is_err(),
                "{field}"
            );
        }
        assert!(validate(&info, PixiResolution::Applied(info.clone())).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut native = info;
            native.binary = PathBuf::from(OsString::from_vec(b"/prefix/\xff/R".to_vec()));
            assert_eq!(deserialize(&serialize(&native, &[])).unwrap().0, native);
        }
    }
}
