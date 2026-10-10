//! Identify an externally activated Pixi environment without running Pixi.

mod probe;
pub(super) mod reexec;

use super::r_source::{RSourceDiagnostic, warning};
use crate::config::PixiRSourceInfo as PixiRTarget;
use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(super) enum PixiResolution {
    NoMatch,
    Rejected(RSourceDiagnostic),
    Applied(PixiRTarget),
}

pub(super) fn resolve(cwd: Option<&Path>) -> PixiResolution {
    resolve_with(cwd, |key| std::env::var_os(key), probe::r_home)
}

#[derive(Deserialize)]
struct Marker {
    manifest_path: PathBuf,
    environment_name: String,
}

fn invalid(reason: &'static str) -> PixiResolution {
    PixiResolution::Rejected(warning(
        "r_source_override.pixi_environment_invalid",
        format!(
            "Warning: The active Pixi environment is invalid: {reason}; trying the next R source override."
        ),
        None,
    ))
}

fn unavailable(reason: &'static str) -> PixiResolution {
    PixiResolution::Rejected(warning(
        "r_source_override.pixi_r_unavailable",
        format!(
            "Warning: R from the active Pixi environment is unavailable: {reason}; trying the next R source override."
        ),
        None,
    ))
}

fn resolve_with(
    cwd: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
    probe: impl Fn(&Path) -> Result<PathBuf, probe::ProbeError>,
) -> PixiResolution {
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
        return match cwd.as_deref().and_then(workspace_manifest) {
            Some(path) => PixiResolution::Rejected(warning(
                "r_source_override.pixi_not_active",
                "Warning: A Pixi workspace was found, but its environment is not active. Start arf with `pixi run arf`; trying the next R source override.",
                Some(path),
            )),
            None => PixiResolution::NoMatch,
        };
    }

    // Prefer the current name even when its value is empty or invalid.
    let manifest = current_manifest.or(legacy_manifest);
    let prefix = env("CONDA_PREFIX");
    let (Some(_), Some(manifest), Some(environment), Some(prefix)) =
        (in_shell, manifest, environment, prefix)
    else {
        return invalid("activation variables are incomplete");
    };
    if manifest.is_empty() || environment.is_empty() || prefix.is_empty() {
        return invalid("an activation identity value is empty");
    }
    let Some(environment) = environment.to_str() else {
        return invalid("the environment name is not valid Unicode");
    };
    let Ok(prefix) = Path::new(&prefix).canonicalize() else {
        return invalid("the prefix cannot be resolved");
    };
    if !prefix.is_dir() {
        return invalid("the prefix is not a directory");
    }
    let Ok(manifest) = Path::new(&manifest).canonicalize() else {
        return invalid("the manifest cannot be resolved");
    };
    if !manifest.is_file() {
        return invalid("the manifest is not a file");
    }
    let Ok(marker_path) = prefix.join("conda-meta/pixi").canonicalize() else {
        return invalid("the environment marker is missing");
    };
    if !marker_path.is_file() || !marker_path.starts_with(&prefix) {
        return invalid("the environment marker is outside the prefix or not a file");
    }
    let marker = std::fs::read(&marker_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Marker>(&bytes).ok());
    let Some(marker) = marker else {
        return invalid("the environment marker cannot be read");
    };
    if marker.environment_name != environment
        || marker.manifest_path.canonicalize().ok().as_ref() != Some(&manifest)
    {
        return invalid("the environment marker does not match activation");
    }

    let Some(binary) = env("PATH").as_deref().and_then(first_r_on_path) else {
        return unavailable("PATH contains no executable R");
    };
    let Ok(binary) = binary.canonicalize() else {
        return unavailable("the PATH executable cannot be resolved");
    };
    if !binary.starts_with(&prefix) {
        return unavailable("the first R on PATH is outside the Pixi prefix");
    }
    let home = match probe(&binary) {
        Ok(home) => home,
        Err(error) => return unavailable(error.description()),
    };
    let Ok(home) = home.canonicalize() else {
        return unavailable("R_HOME cannot be resolved");
    };
    if !home.is_dir() || !home.starts_with(&prefix) {
        return unavailable("R_HOME is outside the Pixi prefix or not a directory");
    }
    let Ok(library) = arf_libr::r_library_path(&home).canonicalize() else {
        return unavailable("the R shared library is missing");
    };
    if !library.is_file() || !library.starts_with(&prefix) {
        return unavailable("the R shared library is outside the Pixi prefix or not a file");
    }
    PixiResolution::Applied(PixiRTarget {
        version: r_base_version(&prefix),
        manifest,
        environment: environment.to_owned(),
        prefix,
        binary,
        home,
        library,
    })
}

fn first_r_on_path(path: &std::ffi::OsStr) -> Option<PathBuf> {
    #[cfg(windows)]
    const R: &str = "R.exe";
    #[cfg(not(windows))]
    const R: &str = "R";
    std::env::split_paths(path)
        .map(|directory| directory.join(R))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
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
