//! Identify an externally activated Pixi environment without running Pixi.

mod probe;

use super::executable::{R_EXECUTABLE, is_executable};
use crate::config::{PixiRSourceInfo, VerifiedRTarget};
use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PixiFailure {
    NotActive {
        workspace_manifest: Option<PathBuf>,
    },
    EnvironmentInvalid {
        reason: &'static str,
        path: Option<PathBuf>,
    },
    RUnavailable {
        reason: &'static str,
        path: Option<PathBuf>,
    },
}

impl std::fmt::Display for PixiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive { workspace_manifest } => {
                if workspace_manifest.is_some() {
                    f.write_str("Pixi is not active for the current workspace")
                } else {
                    f.write_str("No active Pixi environment was found")
                }
            }
            Self::EnvironmentInvalid { reason, .. } => {
                write!(f, "Pixi environment identity is invalid: {reason}")?;
                Ok(())
            }
            Self::RUnavailable { reason, .. } => {
                write!(
                    f,
                    "R from the active Pixi environment is unavailable: {reason}"
                )?;
                Ok(())
            }
        }
    }
}

impl PixiFailure {
    /// Stable identifier of the failure kind; consumers add their own prefix.
    pub(crate) fn code_suffix(&self) -> &'static str {
        match self {
            Self::NotActive { .. } => "pixi_not_active",
            Self::EnvironmentInvalid { .. } => "pixi_environment_invalid",
            Self::RUnavailable { .. } => "pixi_r_unavailable",
        }
    }
}

impl std::error::Error for PixiFailure {}

pub(super) fn resolve_strict(cwd: Option<&Path>) -> Result<PixiRSourceInfo, PixiFailure> {
    resolve_strict_with(cwd, |key| std::env::var_os(key), probe::r_home)
}

#[derive(Deserialize)]
struct Marker {
    manifest_path: PathBuf,
    environment_name: String,
}

fn resolve_strict_with(
    cwd: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
    probe: impl Fn(&Path) -> Result<PathBuf, probe::ProbeError>,
) -> Result<PixiRSourceInfo, PixiFailure> {
    let activation = verify_activation(cwd, &env)?;
    let target = verify_r_target(&activation.prefix, &env, probe)?;
    Ok(PixiRSourceInfo {
        version: r_base_version(&activation.prefix),
        manifest: activation.manifest,
        environment: activation.environment,
        prefix: activation.prefix,
        target,
    })
}

/// The identity of an activated Pixi environment, verified on disk.
struct Activation {
    manifest: PathBuf,
    environment: String,
    prefix: PathBuf,
}

fn verify_activation(
    cwd: Option<&Path>,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Result<Activation, PixiFailure> {
    let in_shell = env("PIXI_IN_SHELL");
    let current_manifest = env("PIXI_WORKSPACE_MANIFEST");
    let legacy_manifest = env("PIXI_PROJECT_MANIFEST");
    let environment = env("PIXI_ENVIRONMENT_NAME");
    // A generic Conda prefix alone is not evidence of Pixi activation.
    if in_shell.is_none()
        && current_manifest.is_none()
        && legacy_manifest.is_none()
        && environment.is_none()
    {
        let cwd = cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok());
        let workspace_manifest = cwd.as_deref().and_then(workspace_manifest);
        return Err(PixiFailure::NotActive { workspace_manifest });
    }

    // Prefer the current name even when its value is empty or invalid.
    let manifest = current_manifest.or(legacy_manifest);
    let prefix = env("CONDA_PREFIX");
    let (Some(_), Some(manifest), Some(environment), Some(prefix)) =
        (in_shell, manifest, environment, prefix)
    else {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "activation variables are incomplete",
            path: None,
        });
    };
    if manifest.is_empty() || environment.is_empty() || prefix.is_empty() {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "an activation identity value is empty",
            path: None,
        });
    }
    let Some(environment) = environment.to_str() else {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the environment name is not valid Unicode",
            path: None,
        });
    };
    let prefix_path = PathBuf::from(prefix);
    let Ok(prefix) = prefix_path.canonicalize() else {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the prefix cannot be resolved",
            path: Some(prefix_path),
        });
    };
    if !prefix.is_dir() {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the prefix is not a directory",
            path: Some(prefix),
        });
    }
    let manifest_path = PathBuf::from(manifest);
    let Ok(manifest) = manifest_path.canonicalize() else {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the manifest cannot be resolved",
            path: Some(manifest_path),
        });
    };
    if !manifest.is_file() {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the manifest is not a file",
            path: Some(manifest),
        });
    }
    let Ok(marker_path) = prefix.join("conda-meta/pixi").canonicalize() else {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the environment marker is missing",
            path: Some(prefix.join("conda-meta/pixi")),
        });
    };
    if !marker_path.is_file() || !marker_path.starts_with(&prefix) {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the environment marker is outside the prefix or not a file",
            path: Some(marker_path),
        });
    }
    let marker = std::fs::read(&marker_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Marker>(&bytes).ok());
    let Some(marker) = marker else {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the environment marker cannot be read",
            path: Some(marker_path),
        });
    };
    if marker.environment_name != environment
        || marker.manifest_path.canonicalize().ok().as_ref() != Some(&manifest)
    {
        return Err(PixiFailure::EnvironmentInvalid {
            reason: "the environment marker does not match activation",
            path: Some(marker_path),
        });
    }
    Ok(Activation {
        manifest,
        environment: environment.to_owned(),
        prefix,
    })
}

fn verify_r_target(
    prefix: &Path,
    env: &impl Fn(&str) -> Option<OsString>,
    probe: impl Fn(&Path) -> Result<PathBuf, probe::ProbeError>,
) -> Result<VerifiedRTarget, PixiFailure> {
    let Some(binary) = env("PATH").as_deref().and_then(first_r_on_path) else {
        return Err(PixiFailure::RUnavailable {
            reason: "PATH contains no executable R",
            path: None,
        });
    };
    let Ok(binary) = binary.canonicalize() else {
        return Err(PixiFailure::RUnavailable {
            reason: "the PATH executable cannot be resolved",
            path: Some(binary),
        });
    };
    if !binary.starts_with(prefix) {
        return Err(PixiFailure::RUnavailable {
            reason: "the first R on PATH is outside the Pixi prefix",
            path: Some(binary),
        });
    }
    let home = match probe(&binary) {
        Ok(home) => home,
        Err(error) => {
            return Err(PixiFailure::RUnavailable {
                reason: error.description(),
                path: Some(binary),
            });
        }
    };
    let Ok(home) = home.canonicalize() else {
        return Err(PixiFailure::RUnavailable {
            reason: "R_HOME cannot be resolved",
            path: Some(home),
        });
    };
    if !home.is_dir() || !home.starts_with(prefix) {
        return Err(PixiFailure::RUnavailable {
            reason: "R_HOME is outside the Pixi prefix or not a directory",
            path: Some(home),
        });
    }
    let Ok(library) = arf_libr::r_library_path(&home).canonicalize() else {
        return Err(PixiFailure::RUnavailable {
            reason: "the R shared library is missing",
            path: Some(arf_libr::r_library_path(&home)),
        });
    };
    if !library.is_file() || !library.starts_with(prefix) {
        return Err(PixiFailure::RUnavailable {
            reason: "the R shared library is outside the Pixi prefix or not a file",
            path: Some(library),
        });
    }
    Ok(VerifiedRTarget {
        binary,
        home,
        library,
    })
}

fn first_r_on_path(path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|directory| directory.join(R_EXECUTABLE))
        .find(|path| is_executable(path))
}

/// CWD discovery is guidance only. Package-only and malformed manifests do
/// not describe a workspace, and must not break R resolution.
fn workspace_manifest(cwd: &Path) -> Option<PathBuf> {
    for (filename, keys) in [
        ("pixi.toml", ["workspace", "project"]),
        (
            "pyproject.toml",
            ["tool.pixi.workspace", "tool.pixi.project"],
        ),
    ] {
        let path = cwd.join(filename);
        let Some(document) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        else {
            continue;
        };
        if keys.iter().any(|key| {
            key.split('.')
                .try_fold(&document, |value, key| value.get(key))
                .is_some_and(toml::Value::is_table)
        }) {
            return Some(path);
        }
    }
    None
}

fn r_base_version(prefix: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Package {
        name: String,
        version: String,
    }
    let entries = std::fs::read_dir(prefix.join("conda-meta")).ok()?;
    entries.filter_map(Result::ok).find_map(|entry| {
        let filename = entry.file_name();
        let filename = filename.to_str()?;
        if !filename.starts_with("r-base-") || !filename.ends_with(".json") {
            return None;
        }
        let package: Package = serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?;
        (package.name == "r-base" && !package.version.is_empty()).then_some(package.version)
    })
}

#[cfg(test)]
mod tests;
