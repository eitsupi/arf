//! R_HOME discovery and rig integration.

use crate::config::RSourceStatus;
use crate::external;
use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug)]
pub(super) struct ResolvedRSource {
    pub(super) status: RSourceStatus,
    pub(super) r_home: Option<PathBuf>,
}

/// Resolve R_HOME from a user-provided path.
///
/// The path can be either:
/// - An installation prefix (e.g., `/opt/R/4.5.2`) containing `bin/R`
/// - The actual R_HOME directory (e.g., `/opt/R/4.5.2/lib/R`)
///
/// If the path contains `bin/R`, we run it with `RHOME` to get the actual R_HOME.
pub(super) fn resolve_r_home_from_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
    // Check if this looks like an installation prefix (has bin/R)
    let r_binary = path.join("bin").join("R");
    if r_binary.exists() {
        // Run `bin/R RHOME` to get the actual R_HOME
        let output = std::process::Command::new(&r_binary)
            .arg("RHOME")
            .output()
            .with_context(|| format!("Failed to run {} RHOME", r_binary.display()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("{} RHOME failed: {}", r_binary.display(), stderr);
        }

        let r_home = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if r_home.is_empty() {
            anyhow::bail!("{} RHOME returned empty result", r_binary.display());
        }

        log::debug!(
            "Resolved R_HOME from installation prefix: {} -> {}",
            path.display(),
            r_home
        );
        return Ok(std::path::PathBuf::from(r_home));
    }

    // Assume the path is already R_HOME
    // Validate by checking for etc/Renviron
    let renviron = path.join("etc").join("Renviron");
    if !renviron.exists() {
        log::warn!(
            "Path {} does not look like R_HOME (missing etc/Renviron). \
             Consider providing the installation prefix instead.",
            path.display()
        );
    }

    Ok(path.to_path_buf())
}

/// Resolve R via rig with a specific version (used for CLI --with-r-version).
pub(super) fn setup_r_via_rig(version_spec: &str) -> Result<ResolvedRSource> {
    if !external::rig::rig_available() {
        return Err(
            anyhow::Error::new(external::rig::RigError::NotInstalled).context(
                "--with-r-version requires rig to be installed.\n\
             Install rig from https://github.com/r-lib/rig",
            ),
        );
    }

    match external::rig::resolve_version(version_spec) {
        Ok(resolved) => resolve_rig_resolution(resolved),
        Err(error) => Err(anyhow::Error::new(error)
            .context(format!("Failed to resolve R version '{version_spec}'"))),
    }
}

/// Set up R from the exact semantic version selected for an override.
///
/// The override resolver has already selected a version from rig's reported
/// version fields. Re-resolving its string would allow a rig name or alias to
/// select a different installation.
pub(super) fn setup_r_via_selected_rig_version(
    selected: &semver::Version,
    versions: &[external::rig::RigVersion],
) -> Result<ResolvedRSource> {
    match external::rig::resolve_selected_version_from_versions(selected, versions) {
        Ok(resolved) => resolve_rig_resolution(resolved),
        Err(error) => anyhow::bail!("Failed to resolve R version '{}': {}", selected, error),
    }
}

fn resolve_rig_resolution(resolved: external::rig::ResolvedVersion) -> Result<ResolvedRSource> {
    log::info!(
        "Using R version {} from {}",
        resolved.version,
        resolved.r_home
    );
    Ok(ResolvedRSource {
        status: RSourceStatus::Rig {
            version: resolved.version,
            override_info: None,
        },
        r_home: Some(PathBuf::from(resolved.r_home)),
    })
}
