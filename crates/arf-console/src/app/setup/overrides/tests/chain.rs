use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn environment_providers_follow_configured_order_and_short_circuit() {
    for (providers, selected) in [
        (
            vec![RSourceOverride::RigProject, RSourceOverride::Pixi],
            "rig-project",
        ),
        (
            vec![RSourceOverride::Pixi, RSourceOverride::RigProject],
            "pixi",
        ),
    ] {
        let result = setup_r_via_overrides_with_environment_providers(
            &providers,
            None,
            || Ok(()),
            || Ok(Vec::new()),
            |_, _| panic!("environment providers do not resolve version files"),
            |_| {
                if selected == "pixi" {
                    Ok(pixi_target())
                } else {
                    Err(PixiFailure::NotActive {
                        workspace_manifest: None,
                    })
                }
            },
            |_| {
                if selected == "rig-project" {
                    Ok(rig_project_target())
                } else {
                    Err(RigProjectFailure::ProjectNotFound {
                        path: "/workspace/rproj.toml".into(),
                    })
                }
            },
        )
        .unwrap();
        match (selected, expect_environment(result).0) {
            ("pixi", RSourceStatus::Pixi { info }) => assert_eq!(info.environment, "analysis"),
            ("rig-project", RSourceStatus::RigProject { info }) => {
                assert_eq!(info.installation_name, "release");
            }
            (_, _) => panic!("provider order selected the wrong environment"),
        }
    }
}

#[test]
fn rig_failure_is_cached_and_does_not_block_later_pixi() {
    for unavailable in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("first.r-version"), "4.4.2").unwrap();
        std::fs::write(temp.path().join("second.r-version"), "4.4.2").unwrap();
        let availability_calls = AtomicUsize::new(0);
        let list_calls = AtomicUsize::new(0);
        let result = setup_r_via_overrides_with_pixi(
            &[
                RSourceOverride::VersionFile {
                    file: "first.r-version".into(),
                },
                RSourceOverride::VersionFile {
                    file: "second.r-version".into(),
                },
                RSourceOverride::Pixi,
            ],
            Some(temp.path()),
            || {
                availability_calls.fetch_add(1, Ordering::SeqCst);
                if unavailable {
                    Err(external::rig::RigError::NotInstalled)
                } else {
                    Ok(())
                }
            },
            || {
                list_calls.fetch_add(1, Ordering::SeqCst);
                Err(external::rig::RigError::CommandFailed("list failed".into()))
            },
            |_, _| panic!("must not resolve rig after failure"),
            |_| Ok(pixi_target()),
        )
        .unwrap();
        let (status, diagnostics) = expect_environment(result);
        assert!(
            matches!(status, RSourceStatus::Pixi { ref info } if info.environment == "analysis")
        );
        assert_eq!(availability_calls.load(Ordering::SeqCst), 1);
        assert_eq!(list_calls.load(Ordering::SeqCst), usize::from(!unavailable));
        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.message.to_lowercase().contains("falling back"))
        );
    }
}

#[test]
fn successful_provider_short_circuits_and_rejected_pixi_continues() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(".r-version"), "4.4.2").unwrap();
    let file = RSourceOverride::VersionFile {
        file: ".r-version".into(),
    };
    let version = rig_version("4.4.2");
    let resolve = |_: &semver::Version, _: &[external::rig::RigVersion]| {
        Ok(ResolvedRSource {
            status: RSourceStatus::Rig {
                version: "4.4.2".into(),
                override_info: None,
            },
            r_home: Some(temp.path().to_path_buf()),
        })
    };
    let result = setup_r_via_overrides_with_pixi(
        &[file.clone(), RSourceOverride::Pixi],
        Some(temp.path()),
        || Ok(()),
        || Ok(vec![version.clone()]),
        resolve,
        |_| panic!("successful file provider must skip Pixi"),
    );
    assert!(matches!(result, Some(OverrideResolution::Applied { .. })));
    let failures = [
        PixiFailure::EnvironmentInvalid {
            reason: "invalid marker",
            path: Some("/prefix/conda-meta/pixi".into()),
        },
        PixiFailure::RUnavailable {
            reason: "R probe timed out",
            path: Some("/prefix/bin/R".into()),
        },
    ];
    let mut summaries = Vec::new();
    for failure in failures {
        let result = setup_r_via_overrides_with_pixi(
            &[RSourceOverride::Pixi, file.clone()],
            Some(temp.path()),
            || Ok(()),
            || Ok(vec![version.clone()]),
            resolve,
            |_| Err(failure.clone()),
        );
        let Some(OverrideResolution::Applied { diagnostics, .. }) = result else {
            panic!("expected file provider")
        };
        assert_eq!(diagnostics.len(), 1);
        summaries.push(serde_json::json!({
            "code": diagnostics[0].code,
            "message": diagnostics[0].message,
            "path": diagnostics[0].path,
        }));
    }
    insta::assert_snapshot!(serde_json::to_string_pretty(&summaries).unwrap(), @r###"
    [
      {
        "code": "r_source_override.pixi_environment_invalid",
        "message": "Warning: The active Pixi environment is invalid: invalid marker; trying the next R source override.",
        "path": null
      },
      {
        "code": "r_source_override.pixi_r_unavailable",
        "message": "Warning: R from the active Pixi environment is unavailable: R probe timed out; trying the next R source override.",
        "path": null
      }
    ]
    "###);
    let result = setup_r_via_overrides_with_pixi(
        &[RSourceOverride::Pixi],
        Some(temp.path()),
        || panic!("must not query rig"),
        || panic!("must not query rig"),
        resolve,
        |_| Ok(pixi_target()),
    );
    assert!(matches!(
        result,
        Some(OverrideResolution::EnvironmentApplied { .. })
    ));
}

#[test]
fn pixi_only_override_resolution_does_not_need_a_base_dir() {
    let result = setup_r_via_overrides_with_pixi(
        &[RSourceOverride::Pixi],
        None,
        || panic!("must not query rig"),
        || panic!("must not query rig"),
        |_, _| panic!("must not query rig"),
        |cwd| {
            assert!(cwd.is_none());
            Err(PixiFailure::NotActive {
                workspace_manifest: None,
            })
        },
    );
    assert!(result.is_none());
}

#[test]
fn command_failed_rig_warning_preserves_reason_without_install_guidance() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("project.r-version"), "4.4.2\n").unwrap();

    let result = setup_r_via_overrides_with(
        &[RSourceOverride::VersionFile {
            file: "project.r-version".into(),
        }],
        Some(temp.path()),
        || {
            Err(external::rig::RigError::CommandFailed(
                "permission denied".to_string(),
            ))
        },
        || panic!("rig versions should not be listed after availability fails"),
        |_, _| panic!("R version should not be resolved after availability fails"),
    )
    .expect("an existing override should produce a fallback");

    let OverrideResolution::Fallback { diagnostics } = result else {
        panic!("a failed rig command should fall back to startup.r_source");
    };
    let message = diagnostic_text(&diagnostics);
    assert!(message.contains("permission denied"));
    assert!(!message.contains("Install rig"));
}

#[test]
fn not_installed_override_falls_through_to_installed_provider() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("first.r-version"), "4.3.0\n").unwrap();
    std::fs::write(temp.path().join("second.r-version"), "4.4.2\n").unwrap();

    let result = setup_with_rig_4_4_2(
        &[
            RSourceOverride::VersionFile {
                file: "first.r-version".into(),
            },
            RSourceOverride::VersionFile {
                file: "second.r-version".into(),
            },
        ],
        temp.path(),
    )
    .unwrap();

    let OverrideResolution::Applied {
        status,
        info,
        diagnostics,
        ..
    } = result
    else {
        panic!("the installed provider should be applied");
    };
    assert!(matches!(*status, RSourceStatus::Rig { .. }));
    assert_eq!(info.file, Path::new("second.r-version"));
    assert_eq!(info.resolved_version, "4.4.2");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        "r_source_override.version_not_installed"
    );
    assert!(diagnostics[0].message.contains("4.3.0"));
    assert!(
        diagnostics[0]
            .message
            .contains("Trying the next R source override")
    );
    assert!(
        !diagnostics[0]
            .message
            .contains("Falling back to startup.r_source")
    );
}

#[test]
fn rig_data_is_fetched_once_across_override_providers() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("first.r-version"), "4.3.0\n").unwrap();
    std::fs::write(temp.path().join("second.r-version"), "4.4.2\n").unwrap();

    let rig_available_calls = Arc::new(AtomicUsize::new(0));
    let list_versions_calls = Arc::new(AtomicUsize::new(0));
    let rig_available_calls_for_closure = Arc::clone(&rig_available_calls);
    let list_versions_calls_for_closure = Arc::clone(&list_versions_calls);

    let result = setup_r_via_overrides_with(
        &[
            RSourceOverride::VersionFile {
                file: "first.r-version".into(),
            },
            RSourceOverride::VersionFile {
                file: "second.r-version".into(),
            },
        ],
        Some(temp.path()),
        move || {
            rig_available_calls_for_closure.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        move || {
            list_versions_calls_for_closure.fetch_add(1, Ordering::Relaxed);
            Ok(vec![rig_version("4.4.2")])
        },
        resolve_installed_4_4_2,
    )
    .unwrap();

    assert!(matches!(result, OverrideResolution::Applied { .. }));
    assert_eq!(rig_available_calls.load(Ordering::Relaxed), 1);
    assert_eq!(list_versions_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn missing_override_files_do_not_fetch_rig_data() {
    let temp = tempfile::tempdir().unwrap();
    let rig_available_calls = Arc::new(AtomicUsize::new(0));
    let list_versions_calls = Arc::new(AtomicUsize::new(0));
    let rig_available_calls_for_closure = Arc::clone(&rig_available_calls);
    let list_versions_calls_for_closure = Arc::clone(&list_versions_calls);

    let result = setup_r_via_overrides_with(
        &[
            RSourceOverride::VersionFile {
                file: "missing-first.r-version".into(),
            },
            RSourceOverride::VersionFile {
                file: "missing-second.r-version".into(),
            },
        ],
        Some(temp.path()),
        move || {
            rig_available_calls_for_closure.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        move || {
            list_versions_calls_for_closure.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        },
        |_, _| panic!("R version resolution should not be attempted"),
    );

    assert!(result.is_none());
    assert_eq!(rig_available_calls.load(Ordering::Relaxed), 0);
    assert_eq!(list_versions_calls.load(Ordering::Relaxed), 0);
}
