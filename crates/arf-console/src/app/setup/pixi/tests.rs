use super::*;
use std::collections::BTreeMap;
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    env: BTreeMap<String, OsString>,
    home: PathBuf,
    binary: PathBuf,
    marker: PathBuf,
}

fn executable(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join("active/pixi.toml");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(
            &manifest,
            r"[workspace]
name = 'active'
",
        )
        .unwrap();
        // Deliberately detached from the workspace, with a named environment.
        let prefix = root.path().join("detached-prefix");
        let home = prefix.join("lib/R");
        let library = arf_libr::r_library_path(&home);
        executable(&library);
        let binary = prefix.join(if cfg!(windows) {
            "Scripts/R.exe"
        } else {
            "bin/R"
        });
        executable(&binary);
        let marker = prefix.join("conda-meta/pixi");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(
            &marker,
            serde_json::to_vec(&serde_json::json!({
                "manifest_path": manifest,
                "environment_name": "analysis",
                "unknown_future_field": [1, 2, 3]
            }))
            .unwrap(),
        )
        .unwrap();
        let env = [
            ("PIXI_IN_SHELL", OsString::new()),
            ("PIXI_WORKSPACE_MANIFEST", manifest.into_os_string()),
            ("PIXI_ENVIRONMENT_NAME", "analysis".into()),
            ("CONDA_PREFIX", prefix.into_os_string()),
            (
                "PATH",
                std::env::join_paths([binary.parent().unwrap()]).unwrap(),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        Self {
            root,
            env,
            home,
            binary,
            marker,
        }
    }

    fn resolve(&self) -> PixiResolution {
        self.resolve_in(Some(self.root.path()))
    }

    fn resolve_in(&self, cwd: Option<&Path>) -> PixiResolution {
        resolve_with(
            cwd,
            |key| self.env.get(key).cloned(),
            |binary| {
                assert_eq!(binary, self.binary.canonicalize().unwrap());
                Ok(self.home.clone())
            },
        )
    }
}

fn rejected(resolution: PixiResolution, code: &'static str) {
    let PixiResolution::Rejected(diagnostic) = resolution else {
        panic!("expected rejection: {code}");
    };
    assert_eq!(diagnostic.code, code);
}

#[test]
fn active_named_detached_environment_preserves_verified_target_and_marker() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.marker).unwrap();
    // CWD points at a different workspace. Active identity must still win.
    std::fs::write(
        fixture.root.path().join("pixi.toml"),
        r"[workspace]
name='other'",
    )
    .unwrap();
    let PixiResolution::Applied(target) = fixture.resolve() else {
        panic!("expected an applied Pixi target");
    };
    assert_eq!(target.environment, "analysis");
    assert_eq!(target.binary, fixture.binary.canonicalize().unwrap());
    assert_eq!(target.home, fixture.home.canonicalize().unwrap());
    assert_eq!(
        target.library,
        arf_libr::r_library_path(&fixture.home)
            .canonicalize()
            .unwrap()
    );
    assert_eq!(target.version, None);
    assert_eq!(std::fs::read(&fixture.marker).unwrap(), before);
    assert!(!target.binary.starts_with(&target.home));
}

#[test]
fn missing_empty_and_legacy_activation_values_are_distinct() {
    for key in [
        "PIXI_IN_SHELL",
        "PIXI_WORKSPACE_MANIFEST",
        "PIXI_ENVIRONMENT_NAME",
        "CONDA_PREFIX",
    ] {
        let mut fixture = Fixture::new();
        fixture.env.remove(key);
        rejected(
            fixture.resolve(),
            "r_source_override.pixi_environment_invalid",
        );
        if key != "PIXI_IN_SHELL" {
            let mut fixture = Fixture::new();
            fixture.env.insert(key.into(), OsString::new());
            rejected(
                fixture.resolve(),
                "r_source_override.pixi_environment_invalid",
            );
        }
    }
    let mut fixture = Fixture::new();
    let manifest = fixture.env.remove("PIXI_WORKSPACE_MANIFEST").unwrap();
    fixture.env.insert("PIXI_PROJECT_MANIFEST".into(), manifest);
    assert!(matches!(fixture.resolve(), PixiResolution::Applied(_)));
    for invalid in ["", "nonexistent.toml"] {
        fixture
            .env
            .insert("PIXI_WORKSPACE_MANIFEST".into(), invalid.into());
        rejected(
            fixture.resolve(),
            "r_source_override.pixi_environment_invalid",
        );
    }
}

#[test]
fn generic_conda_and_absent_signals_are_silent() {
    let fixture = Fixture::new();
    for conda in [None, Some(OsString::from("/conda"))] {
        assert!(matches!(
            resolve_with(
                Some(fixture.root.path()),
                |key| {
                    if key == "CONDA_PREFIX" {
                        conda.clone()
                    } else {
                        None
                    }
                },
                |_| panic!("must not probe inactive environment")
            ),
            PixiResolution::NoMatch
        ));
    }
}

#[test]
fn guidance_detects_only_exact_cwd_workspace_manifests() {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(root.path().join("pixi.toml"), "[workspace]").unwrap();
    assert_eq!(workspace_manifest(&child), None);
    for (filename, text, matched) in [
        ("pixi.toml", "[workspace]", true),
        ("pixi.toml", "[project]", true),
        ("pixi.toml", "[package]", false),
        ("pixi.toml", "invalid [", false),
        (
            "pyproject.toml",
            r"[project]
name='python'",
            false,
        ),
        ("pyproject.toml", "[tool.pixi.package]", false),
        ("pyproject.toml", "[tool.pixi.project]", true),
        ("pyproject.toml", "[tool.pixi.workspace]", true),
    ] {
        let path = child.join(filename);
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            workspace_manifest(&child),
            matched.then_some(path.clone()),
            "{filename}: {text}"
        );
        let result = resolve_with(Some(&child), |_| None, |_| panic!("must not probe"));
        if matched {
            rejected(result, "r_source_override.pixi_not_active");
        } else {
            assert!(matches!(result, PixiResolution::NoMatch));
        }
        std::fs::remove_file(path).unwrap();
    }
    std::fs::write(child.join("pixi.toml"), "[workspace]").unwrap();
    std::fs::write(child.join("pyproject.toml"), "[tool.pixi.workspace]").unwrap();
    assert_eq!(workspace_manifest(&child), Some(child.join("pixi.toml")));
}

#[test]
fn invalid_markers_are_never_repaired_or_deleted() {
    let fixture = Fixture::new();
    for bytes in [
        b"{".as_slice(),
        br#"{"environment_name":"analysis"}"#,
        br#"{"environment_name":"other","manifest_path":"/missing"}"#,
    ] {
        std::fs::write(&fixture.marker, bytes).unwrap();
        rejected(
            fixture.resolve(),
            "r_source_override.pixi_environment_invalid",
        );
        assert_eq!(std::fs::read(&fixture.marker).unwrap(), bytes);
    }
    std::fs::remove_file(&fixture.marker).unwrap();
    rejected(
        fixture.resolve(),
        "r_source_override.pixi_environment_invalid",
    );
    assert!(!fixture.marker.exists());
}

#[test]
fn first_path_r_outside_prefix_is_rejected_without_probing_later_r() {
    let mut fixture = Fixture::new();
    let prefix = PathBuf::from(&fixture.env["CONDA_PREFIX"]);
    // A sibling sharing the prefix's text is not contained by its components.
    let outside = prefix
        .with_file_name("detached-prefix-other")
        .join(fixture.binary.file_name().unwrap());
    executable(&outside);
    fixture.env.insert(
        "PATH".into(),
        std::env::join_paths([outside.parent().unwrap(), fixture.binary.parent().unwrap()])
            .unwrap(),
    );
    rejected(
        resolve_with(
            Some(fixture.root.path()),
            |key| fixture.env.get(key).cloned(),
            |_| panic!("must not probe outside R"),
        ),
        "r_source_override.pixi_r_unavailable",
    );
    fixture.env.remove("PATH");
    rejected(fixture.resolve(), "r_source_override.pixi_r_unavailable");
}

#[test]
fn missing_library_outside_home_and_probe_failure_reject() {
    let fixture = Fixture::new();
    let outside = fixture.root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    for result in [
        Ok(outside),
        Err(probe::ProbeError::Timeout),
        Err(probe::ProbeError::OutputLimit),
    ] {
        rejected(
            resolve_with(
                Some(fixture.root.path()),
                |key| fixture.env.get(key).cloned(),
                |_| result.clone(),
            ),
            "r_source_override.pixi_r_unavailable",
        );
    }
    std::fs::remove_file(arf_libr::r_library_path(&fixture.home)).unwrap();
    rejected(fixture.resolve(), "r_source_override.pixi_r_unavailable");
}

#[test]
fn version_metadata_is_optional_and_best_effort() {
    let fixture = Fixture::new();
    let metadata = fixture
        .marker
        .parent()
        .unwrap()
        .join("r-base-4.5.3-build.json");
    for (text, expected) in [
        ("{", None),
        (r#"{"name":"other","version":"9"}"#, None),
        (r#"{"name":"r-base","version":"4.5.3"}"#, Some("4.5.3")),
    ] {
        std::fs::write(&metadata, text).unwrap();
        let PixiResolution::Applied(target) = fixture.resolve() else {
            panic!("expected success")
        };
        assert_eq!(target.version.as_deref(), expected);
    }
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_prefix_validation() {
    use std::os::unix::fs::symlink;
    for escaped in ["marker", "binary", "library"] {
        let fixture = Fixture::new();
        let path = match escaped {
            "marker" => fixture.marker.clone(),
            "binary" => fixture.binary.clone(),
            _ => arf_libr::r_library_path(&fixture.home),
        };
        let outside = fixture.root.path().join("outside");
        std::fs::rename(&path, &outside).unwrap();
        symlink(&outside, &path).unwrap();
        let code = if escaped == "marker" {
            "r_source_override.pixi_environment_invalid"
        } else {
            "r_source_override.pixi_r_unavailable"
        };
        rejected(fixture.resolve(), code);
    }
}
