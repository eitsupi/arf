mod chain;
mod disclosure;
mod files;
mod messages;

use super::*;
use crate::config::VerifiedRTarget;

fn pixi_target() -> PixiRSourceInfo {
    PixiRSourceInfo {
        manifest: "/workspace/pixi.toml".into(),
        environment: "analysis".into(),
        prefix: "/prefix".into(),
        target: VerifiedRTarget {
            binary: "/prefix/Scripts/R.exe".into(),
            home: "/prefix/lib/R".into(),
            library: "/prefix/lib/R/lib/libR.so".into(),
        },
        version: None,
    }
}

fn rig_project_target() -> RigProjectRSourceInfo {
    RigProjectRSourceInfo {
        root: "/workspace".into(),
        manifest: "/workspace/rproj.toml".into(),
        target: VerifiedRTarget {
            binary: "/workspace/.rvenv/bin/R".into(),
            home: "/rig/R-4.6".into(),
            library: "/rig/R-4.6/lib/libR.so".into(),
        },
        installation_name: "release".into(),
    }
}

fn rig_version(version: &str) -> external::rig::RigVersion {
    external::rig::RigVersion {
        name: version.to_owned(),
        default: true,
        version: version.to_owned(),
        aliases: Vec::new(),
        path: format!("/opt/R/{version}"),
        binary: format!("/opt/R/{version}/bin/R"),
    }
}

fn diagnostic_text(diagnostics: &[RSourceDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolve a request for R 4.4.2 against a single installed rig version.
fn resolve_installed_4_4_2(
    selected: &semver::Version,
    versions: &[external::rig::RigVersion],
) -> Result<ResolvedRSource> {
    assert_eq!(selected, &semver::Version::new(4, 4, 2));
    Ok(ResolvedRSource {
        status: RSourceStatus::Rig {
            version: versions[0].version.clone(),
            override_info: None,
        },
        r_home: Some(PathBuf::from("/tmp/r-home")),
    })
}

/// Run the override chain with rig reporting R 4.4.2 as the only installed version.
fn setup_with_rig_4_4_2(
    overrides: &[RSourceOverride],
    base_dir: &Path,
) -> Option<OverrideResolution> {
    setup_r_via_overrides_with(
        overrides,
        Some(base_dir),
        || Ok(()),
        || Ok(vec![rig_version("4.4.2")]),
        resolve_installed_4_4_2,
    )
}

/// Run the override chain where no provider may reach rig, and return the
/// text of the diagnostics from the resulting fallback.
fn fallback_diagnostic_text(overrides: &[RSourceOverride], base_dir: &Path) -> String {
    let result = setup_r_via_overrides_with(
        overrides,
        Some(base_dir),
        || panic!("rig should not be queried for an unusable provider value"),
        || panic!("installed versions should not be queried for an unusable provider value"),
        |_, _| panic!("version resolution should not be attempted"),
    )
    .expect("an existing override should produce a fallback");
    let OverrideResolution::Fallback { diagnostics } = result else {
        panic!("an unusable provider value must not be applied");
    };
    diagnostic_text(&diagnostics)
}

/// Unpack a resolution selected from a version request.
fn expect_applied(result: OverrideResolution) -> (RSourceOverrideInfo, Vec<RSourceDiagnostic>) {
    match result {
        OverrideResolution::Applied {
            info, diagnostics, ..
        } => (info, diagnostics),
        OverrideResolution::EnvironmentApplied { .. } => panic!("unexpected environment target"),
        OverrideResolution::Fallback { .. } => panic!("a provider should have been applied"),
    }
}

/// Unpack a resolution selected from an environment provider.
fn expect_environment(result: OverrideResolution) -> (RSourceStatus, Vec<RSourceDiagnostic>) {
    match result {
        OverrideResolution::EnvironmentApplied {
            status,
            diagnostics,
        } => (*status, diagnostics),
        OverrideResolution::Applied { .. } => panic!("unexpected version-request target"),
        OverrideResolution::Fallback { .. } => panic!("an environment provider should apply"),
    }
}

fn setup_r_via_overrides_with_pixi<FAvailable, FList, FResolve>(
    overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    rig_available: FAvailable,
    list_versions: FList,
    resolve_selected_rig_version: FResolve,
    resolve_pixi: impl Fn(Option<&Path>) -> std::result::Result<PixiRSourceInfo, PixiFailure>,
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
        resolve_pixi,
        resolve_rig_project,
    )
}
