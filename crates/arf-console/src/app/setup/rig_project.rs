//! Read-only resolution of a synchronized rig project at the current directory.

use serde::Deserialize;
use serde::Deserializer;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{RigProjectRSourceInfo, VerifiedRTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RigProjectFailure {
    ProjectNotFound {
        path: PathBuf,
    },
    StatusUnavailable {
        reason: &'static str,
        path: Option<PathBuf>,
    },
    StatusInvalid {
        reason: &'static str,
        path: Option<PathBuf>,
    },
    NotSynced {
        reason: &'static str,
        path: Option<PathBuf>,
    },
    RUnavailable {
        reason: &'static str,
        path: Option<PathBuf>,
    },
}

impl fmt::Display for RigProjectFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectNotFound { .. } => {
                f.write_str("the current directory does not contain a rig project")
            }
            Self::StatusUnavailable { reason, .. } => {
                write!(f, "rig project status is unavailable: {reason}")
            }
            Self::StatusInvalid { reason, .. } => {
                write!(f, "rig project status is invalid: {reason}")
            }
            Self::NotSynced { reason, .. } => {
                write!(f, "the rig project is not synchronized: {reason}")
            }
            Self::RUnavailable { reason, .. } => {
                write!(f, "the synchronized rig project R is unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for RigProjectFailure {}

pub(crate) fn resolve_strict() -> Result<RigProjectRSourceInfo, RigProjectFailure> {
    let cwd = std::env::current_dir().map_err(|_| RigProjectFailure::ProjectNotFound {
        path: PathBuf::from("."),
    })?;
    resolve_strict_at(&cwd, run_status_command, probe_wrapper)
}

fn resolve_strict_at(
    cwd: &Path,
    run_status: impl FnOnce(Command) -> Result<Vec<u8>, StatusCommandFailure>,
    probe: impl FnOnce(Command) -> Result<PathBuf, arf_libr::RHomeProbeError>,
) -> Result<RigProjectRSourceInfo, RigProjectFailure> {
    let manifest = cwd.join("rproj.toml");
    if !manifest.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(RigProjectFailure::ProjectNotFound { path: manifest });
    }
    let root = cwd
        .canonicalize()
        .map_err(|_| RigProjectFailure::ProjectNotFound {
            path: cwd.to_path_buf(),
        })?;
    let manifest = root.join("rproj.toml");
    let wrapper = rig_wrapper_path(&root);

    let mut status_command = Command::new("rig");
    status_command
        .args(["proj", "status", "--json"])
        .current_dir(&root);
    let output = run_status(status_command).map_err(|failure| match failure {
        StatusCommandFailure::Unavailable => RigProjectFailure::StatusUnavailable {
            reason: "the status command could not be run",
            path: Some(root.clone()),
        },
        StatusCommandFailure::Timeout => RigProjectFailure::StatusUnavailable {
            reason: "the status command exceeded its time limit",
            path: Some(root.clone()),
        },
        StatusCommandFailure::OutputLimit => RigProjectFailure::StatusUnavailable {
            reason: "the status command exceeded its output limit",
            path: Some(root.clone()),
        },
        StatusCommandFailure::Failed => RigProjectFailure::StatusUnavailable {
            reason: "the status command exited unsuccessfully",
            path: Some(root.clone()),
        },
    })?;
    let status: RigProjectStatus =
        serde_json::from_slice(&output).map_err(|_| RigProjectFailure::StatusInvalid {
            reason: "the status output is not valid rig project JSON",
            path: Some(root.clone()),
        })?;
    validate_status(&status, &root)?;

    if !is_executable_file(&wrapper) {
        return Err(RigProjectFailure::RUnavailable {
            reason: "the project R wrapper is missing or not executable",
            path: Some(wrapper),
        });
    }

    let mut probe_command = Command::new(&wrapper);
    probe_command.current_dir(&root).env_remove("R_HOME");
    let home = probe(probe_command).map_err(|error| RigProjectFailure::RUnavailable {
        reason: error.description(),
        path: Some(wrapper.clone()),
    })?;
    // The wrapper is run with `root` as its working directory, so relative
    // RHOME output must be interpreted against that same fixed directory.
    let home = if home.is_absolute() {
        home
    } else {
        root.join(home)
    };
    let home = home
        .canonicalize()
        .map_err(|_| RigProjectFailure::RUnavailable {
            reason: "R_HOME cannot be resolved",
            path: Some(home),
        })?;
    if !home.is_dir() {
        return Err(RigProjectFailure::RUnavailable {
            reason: "R_HOME is not a directory",
            path: Some(home),
        });
    }
    let library_path = arf_libr::r_library_path(&home);
    let library = library_path
        .canonicalize()
        .map_err(|_| RigProjectFailure::RUnavailable {
            reason: "the R shared library is missing",
            path: Some(library_path.clone()),
        })?;
    if !library.is_file() {
        return Err(RigProjectFailure::RUnavailable {
            reason: "the R shared library is not a file",
            path: Some(library),
        });
    }

    let installation_name = match status.sync {
        Nullable::Value(sync) => sync.r_version,
        Nullable::Null => unreachable!("validated status has a non-null sync section"),
    };
    Ok(RigProjectRSourceInfo {
        root,
        manifest,
        target: VerifiedRTarget {
            binary: wrapper,
            home,
            library,
        },
        installation_name,
    })
}

fn validate_status(
    status: &RigProjectStatus,
    expected_root: &Path,
) -> Result<(), RigProjectFailure> {
    if matches!(status.manifest, Nullable::Null) {
        return Err(not_synced("the project manifest is missing", expected_root));
    }
    if matches!(status.lock, Nullable::Null) {
        return Err(not_synced("the project lock is missing", expected_root));
    }
    let sync = match &status.sync {
        Nullable::Null => {
            return Err(not_synced(
                "the project environment is missing",
                expected_root,
            ));
        }
        Nullable::Value(sync) => sync,
    };
    if !status.warnings.is_empty() {
        return Err(not_synced(
            "rig reported warnings while checking synchronization",
            expected_root,
        ));
    }
    if !sync.up_to_date {
        return Err(not_synced(
            "rig reports that the environment needs synchronization",
            expected_root,
        ));
    }
    if !matches!(sync.reason, Nullable::Null) {
        return Err(not_synced(
            "rig reports a synchronization reason",
            expected_root,
        ));
    }
    if sync.r_version.trim().is_empty() {
        return Err(RigProjectFailure::StatusInvalid {
            reason: "the synchronized R installation name is empty",
            path: Some(expected_root.to_path_buf()),
        });
    }

    let returned_root = if status.root.is_absolute() {
        status.root.clone()
    } else {
        expected_root.join(&status.root)
    };
    let canonical_returned_root =
        returned_root
            .canonicalize()
            .map_err(|_| RigProjectFailure::StatusInvalid {
                reason: "the reported project root cannot be resolved",
                path: Some(returned_root.clone()),
            })?;
    if canonical_returned_root != expected_root {
        return Err(RigProjectFailure::StatusInvalid {
            reason: "rig resolved a different project root",
            path: Some(canonical_returned_root),
        });
    }
    Ok(())
}

fn not_synced(reason: &'static str, path: &Path) -> RigProjectFailure {
    RigProjectFailure::NotSynced {
        reason,
        path: Some(path.to_path_buf()),
    }
}

fn run_status_command(command: Command) -> Result<Vec<u8>, StatusCommandFailure> {
    let output = arf_libr::run_bounded_command(command).map_err(|error| match error {
        arf_libr::BoundedProcessError::Unavailable => StatusCommandFailure::Unavailable,
        arf_libr::BoundedProcessError::Timeout => StatusCommandFailure::Timeout,
        arf_libr::BoundedProcessError::OutputLimit => StatusCommandFailure::OutputLimit,
    })?;
    if !output.status.success() {
        return Err(StatusCommandFailure::Failed);
    }
    Ok(output.stdout)
}

fn probe_wrapper(command: Command) -> Result<PathBuf, arf_libr::RHomeProbeError> {
    arf_libr::probe_r_home(command)
}

fn rig_wrapper_path(root: &Path) -> PathBuf {
    #[cfg(windows)]
    const R_EXECUTABLE: &str = "R.exe";
    #[cfg(not(windows))]
    const R_EXECUTABLE: &str = "R";
    root.join(".rvenv").join("bin").join(R_EXECUTABLE)
}

fn is_executable_file(path: &Path) -> bool {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusCommandFailure {
    Unavailable,
    Timeout,
    OutputLimit,
    Failed,
}

#[derive(Deserialize)]
struct RigProjectStatus {
    root: PathBuf,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    manifest: Nullable<BTreeMap<String, serde_json::Value>>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    lock: Nullable<BTreeMap<String, serde_json::Value>>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    sync: Nullable<SyncStatus>,
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct SyncStatus {
    r_version: String,
    up_to_date: bool,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    reason: Nullable<String>,
}

/// A required JSON field that may explicitly contain `null`.
#[derive(Debug, PartialEq, Eq)]
enum Nullable<T> {
    Null,
    Value(T),
}

fn deserialize_required_nullable<'de, T, D>(deserializer: D) -> Result<Nullable<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer)
        .map(|value| value.map_or(Nullable::Null, Nullable::Value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn make_project() -> (TempDir, PathBuf, TempDir) {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("rproj.toml"), "[project]\n").unwrap();
        let root = project.path().canonicalize().unwrap();
        let wrapper = rig_wrapper_path(&root);
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = tempfile::tempdir().unwrap();
        let library = arf_libr::r_library_path(home.path());
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&library, "R shared library fixture").unwrap();
        (project, wrapper, home)
    }

    fn status_value(root: &Path) -> serde_json::Value {
        serde_json::json!({
            "root": root,
            "manifest": {"name": "demo", "version": "0.1.0"},
            "lock": {"version": 1, "targets": []},
            "sync": {"r_version": "4.6-arm64", "up_to_date": true, "reason": null},
            "warnings": [],
            "future_field": "accepted"
        })
    }

    fn status_json(root: &Path) -> Vec<u8> {
        serde_json::to_vec(&status_value(root)).unwrap()
    }

    fn run_with_status(
        bytes: Vec<u8>,
        expected_root: PathBuf,
    ) -> impl FnOnce(Command) -> Result<Vec<u8>, StatusCommandFailure> {
        move |command| {
            assert_eq!(command.get_program(), OsStr::new("rig"));
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                ["proj", "status", "--json"]
            );
            assert_eq!(command.get_current_dir(), Some(expected_root.as_path()));
            Ok(bytes)
        }
    }

    #[test]
    fn resolves_synced_project_and_preserves_the_wrapper_target() {
        let (project, wrapper, home) = make_project();
        let relative_home = PathBuf::from("..").join(home.path().file_name().unwrap());
        let info = resolve_strict_at(
            project.path(),
            run_with_status(
                status_json(project.path()),
                project.path().canonicalize().unwrap(),
            ),
            |command| {
                assert_eq!(command.get_program(), wrapper.as_os_str());
                assert_eq!(
                    command.get_current_dir(),
                    Some(project.path().canonicalize().unwrap().as_path())
                );
                assert!(
                    command
                        .get_envs()
                        .any(|(name, value)| { name == OsStr::new("R_HOME") && value.is_none() })
                );
                Ok(relative_home)
            },
        )
        .unwrap();
        assert_eq!(info.root, project.path().canonicalize().unwrap());
        assert_eq!(info.manifest, info.root.join("rproj.toml"));
        assert_eq!(info.target.binary, wrapper);
        assert_eq!(info.target.home, home.path().canonicalize().unwrap());
        assert_eq!(
            info.target.library,
            arf_libr::r_library_path(&info.target.home)
        );
        assert_eq!(info.installation_name, "4.6-arm64");
    }

    #[test]
    fn skips_status_command_outside_a_direct_project() {
        let cwd = tempfile::tempdir().unwrap();
        let error = resolve_strict_at(
            cwd.path(),
            |_| panic!("status command must not run"),
            |_| panic!("R probe must not run"),
        )
        .unwrap_err();
        assert!(matches!(error, RigProjectFailure::ProjectNotFound { .. }));
    }

    #[test]
    fn rejects_null_or_unsynchronized_sections_and_warnings() {
        let (project, _, _) = make_project();
        let cases = [
            ("manifest", serde_json::Value::Null),
            ("lock", serde_json::Value::Null),
            ("sync", serde_json::Value::Null),
            ("warnings", serde_json::json!(["sync check failed"])),
        ];
        for (field, value) in cases {
            let mut json = status_value(project.path());
            json[field] = value;
            let status: RigProjectStatus = serde_json::from_value(json).unwrap();
            assert!(matches!(
                validate_status(&status, &project.path().canonicalize().unwrap()),
                Err(RigProjectFailure::NotSynced { .. })
            ));
        }
        for field in ["up_to_date", "reason"] {
            let mut json = status_value(project.path());
            if field == "up_to_date" {
                json["sync"][field] = serde_json::Value::Bool(false);
            } else {
                json["sync"][field] = serde_json::Value::String("stale".to_owned());
            }
            let status: RigProjectStatus = serde_json::from_value(json).unwrap();
            assert!(matches!(
                validate_status(&status, &project.path().canonicalize().unwrap()),
                Err(RigProjectFailure::NotSynced { .. })
            ));
        }
    }

    #[test]
    fn rejects_missing_required_fields_wrong_types_and_root_redirects() {
        let (project, _, _) = make_project();
        let valid = status_value(project.path());
        let mut missing_reason = valid.clone();
        missing_reason["sync"]
            .as_object_mut()
            .unwrap()
            .remove("reason");
        let mut missing_warnings = valid.clone();
        missing_warnings.as_object_mut().unwrap().remove("warnings");
        let mut wrong_type = valid.clone();
        wrong_type["sync"]["r_version"] = serde_json::Value::Number(7.into());
        for (field, invalid) in [
            ("sync.reason", missing_reason),
            ("warnings", missing_warnings),
            ("sync.r_version", wrong_type),
        ] {
            assert!(
                serde_json::from_value::<RigProjectStatus>(invalid).is_err(),
                "{field} must be required and typed"
            );
        }
        let mut redirected = valid;
        redirected["root"] = serde_json::json!(project.path().parent().unwrap());
        let status: RigProjectStatus = serde_json::from_value(redirected).unwrap();
        assert!(matches!(
            validate_status(&status, &project.path().canonicalize().unwrap()),
            Err(RigProjectFailure::StatusInvalid { .. })
        ));
    }

    #[test]
    fn rejects_missing_wrapper_and_invalid_library_target() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("rproj.toml"), "[project]\n").unwrap();
        let error = resolve_strict_at(
            project.path(),
            run_with_status(
                status_json(project.path()),
                project.path().canonicalize().unwrap(),
            ),
            |_| panic!("R probe must not run without a wrapper"),
        )
        .unwrap_err();
        assert!(matches!(error, RigProjectFailure::RUnavailable { .. }));
    }

    #[test]
    fn rejects_malformed_status_wrapper_probe_and_missing_library() {
        let (project, _, _home) = make_project();
        let status_failed = resolve_strict_at(
            project.path(),
            |_| Err(StatusCommandFailure::Failed),
            |_| panic!("R probe must not run after status command failure"),
        )
        .unwrap_err();
        assert!(matches!(
            status_failed,
            RigProjectFailure::StatusUnavailable { .. }
        ));

        let malformed = resolve_strict_at(
            project.path(),
            run_with_status(b"not JSON".to_vec(), project.path().canonicalize().unwrap()),
            |_| panic!("R probe must not run after invalid status output"),
        )
        .unwrap_err();
        assert!(matches!(malformed, RigProjectFailure::StatusInvalid { .. }));

        let probe_failed = resolve_strict_at(
            project.path(),
            run_with_status(
                status_json(project.path()),
                project.path().canonicalize().unwrap(),
            ),
            |_| Err(arf_libr::RHomeProbeError::Failed),
        )
        .unwrap_err();
        assert!(matches!(
            probe_failed,
            RigProjectFailure::RUnavailable { .. }
        ));

        let empty_home = tempfile::tempdir().unwrap();
        let missing_library = resolve_strict_at(
            project.path(),
            run_with_status(
                status_json(project.path()),
                project.path().canonicalize().unwrap(),
            ),
            |_| Ok(empty_home.path().to_path_buf()),
        )
        .unwrap_err();
        assert!(matches!(
            missing_library,
            RigProjectFailure::RUnavailable { .. }
        ));
    }
}
