use super::*;
use crate::app::explicit_r_source::RSourceOrigin;

#[test]
fn pixi_report_retains_target_without_override_file() {
    let info = crate::config::PixiRSourceInfo {
        manifest: "/workspace/pixi.toml".into(),
        environment: "analysis".to_owned(),
        prefix: "/prefix".into(),
        target: crate::config::VerifiedRTarget {
            binary: "/prefix/bin/R".into(),
            home: "/prefix/lib/R".into(),
            library: "/prefix/lib/R/lib/libR.so".into(),
        },
        version: Some("4.5.1".to_owned()),
    };
    for override_state in [
        RSourceOverrideState::ShadowedByCli,
        RSourceOverrideState::Disabled,
    ] {
        let report = RSourceResolutionReport::from_environment(
            RSourceStatus::Pixi { info: info.clone() },
            Vec::new(),
            RSourceSelectionRoute::Explicit,
            override_state,
        );

        assert_eq!(report.selection_route, RSourceSelectionRoute::Explicit);
        assert_eq!(report.override_state, override_state);
        assert_eq!(report.r_home.as_deref(), Some(info.target.home.as_path()));
        assert!(
            matches!(report.status, RSourceStatus::Pixi { info: ref target } if target == &info)
        );
        assert!(report.override_provider.is_none());
        assert!(report.file.is_none());
        assert!(report.override_info().is_none());
    }

    // The manifest is provider identity, not a version file read by the override.
    let report = RSourceResolutionReport::from_environment(
        RSourceStatus::Pixi { info },
        Vec::new(),
        RSourceSelectionRoute::Override,
        RSourceOverrideState::Applied,
    );
    assert_eq!(report.override_provider.as_deref(), Some("pixi"));
    assert!(report.file.is_none());
}

#[test]
fn fallback_explicit_path_is_masked_only_for_human_output() {
    let _guard = crate::test_utils::lock_env();
    let home = dirs::home_dir().expect("test requires a home directory");
    let path = home.join("arf-fallback-R");
    let mut report = RSourceResolutionReport::from_status(
        RSourceStatus::ExplicitPath { path: path.clone() },
        Some(path.clone()),
        RSourceOverrideState::Failed,
    );
    report
        .diagnostics
        .push(warning("r_source_override.fallback", "fallback", None));
    finalize_fallback_diagnostic(&mut report);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.path.as_ref(), Some(&path));
    assert!(diagnostic.message.contains(&path.display().to_string()));
    let human = display_message(diagnostic);
    assert!(human.contains("path (~"), "{human}");
    assert!(!human.contains(&home.display().to_string()), "{human}");
}

#[test]
fn display_message_masks_its_own_path_and_leaves_pathless_messages_unchanged() {
    // display_message reads HOME indirectly through dirs::home_dir and mask_home_path.
    let _guard = crate::test_utils::lock_env();
    let home = dirs::home_dir().expect("test requires a home directory");
    let path = home.join("arf").join("broken.toml");
    let diagnostic = RSourceDiagnostic {
        code: "config.parse_failed".to_owned(),
        severity: "warning",
        message: format!(
            r#"Failed to load config from {}: TOML parse error"#,
            path.display()
        ),
        path: Some(path.clone()),
    };
    let pathless = RSourceDiagnostic {
        code: "r_discovery.failed".to_owned(),
        severity: "warning",
        message: r#"R automatic discovery failed."#.to_owned(),
        path: None,
    };

    // Assert the masking took effect rather than comparing against
    // `mask_home_path` again, which would also pass if it stopped masking.
    let masked = display_message(&diagnostic);
    assert!(
        masked.starts_with("Failed to load config from ~"),
        "{masked}"
    );
    assert!(!masked.contains(&home.display().to_string()), "{masked}");
    assert!(
        masked.ends_with("broken.toml: TOML parse error"),
        "{masked}"
    );

    assert_eq!(display_message(&pathless), pathless.message);
}

#[test]
fn explicit_rig_mode_reports_command_failure_without_install_guidance() {
    let result = setup_r_fallback_with(
        &RSource::Mode(RSourceMode::Rig),
        RSourceOverrideState::NotConfigured,
        || {
            Err(external::rig::RigError::CommandFailed(
                "permission denied".to_string(),
            ))
        },
        |_| panic!("rig's default version should not be resolved"),
    );

    let error = result.expect_err("rig command failure should reject explicit rig mode");
    let message = error.to_string();
    assert!(message.contains("permission denied"));
    assert!(!message.contains("Install rig"));
}

fn path_source(path: &Path) -> RSource {
    RSource::Path {
        path: path.to_path_buf(),
    }
}

fn setup_r(
    r_source: &RSource,
    r_source_overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    cli_r_home: Option<&Path>,
    no_r_source_overrides: bool,
) -> Result<RSourceResolutionReport> {
    let request = cli_r_home.map(|home| ExplicitRSourceRequest {
        source: ExplicitRSource::Home(home.to_path_buf()),
        origin: RSourceOrigin::Cli,
    });
    resolve_r_source(
        r_source,
        r_source_overrides,
        base_dir,
        request.as_ref(),
        no_r_source_overrides,
    )
}

#[test]
fn resolve_r_source_does_not_mutate_r_home() {
    // Hold the lock to keep concurrent environment writers out while reading R_HOME.
    let _guard = crate::test_utils::lock_env();
    let original = std::env::var_os("R_HOME");
    let temp = tempfile::tempdir().unwrap();

    let explicit_request = ExplicitRSourceRequest {
        source: ExplicitRSource::Home(temp.path().to_path_buf()),
        origin: RSourceOrigin::Cli,
    };
    let report = resolve_r_source(
        &path_source(temp.path()),
        &[],
        None,
        Some(&explicit_request),
        false,
    )
    .unwrap();

    assert_eq!(report.r_home.as_deref(), Some(temp.path()));
    assert_eq!(std::env::var_os("R_HOME"), original);
}

#[test]
fn path_fallback_reports_path_without_r_home() {
    let without_rig = setup_r_fallback_with(
        &RSource::Mode(RSourceMode::Auto),
        RSourceOverrideState::NotConfigured,
        || Err(external::rig::RigError::NotInstalled),
        |_| panic!("rig's default version should not be resolved"),
    )
    .unwrap();
    let without_default = setup_r_fallback_with(
        &RSource::Mode(RSourceMode::Auto),
        RSourceOverrideState::NotConfigured,
        || Ok(()),
        |_| Err(external::rig::RigError::NoDefaultVersion),
    )
    .unwrap();

    for report in [without_rig, without_default] {
        assert!(matches!(report.status, RSourceStatus::Path));
        assert!(report.r_home.is_none());
    }
}

#[test]
fn path_r_home_discovery_failure_is_reported_deterministically() {
    let mut report = RSourceResolutionReport::from_status(
        RSourceStatus::Path,
        None,
        RSourceOverrideState::Disabled,
    );

    resolve_path_r_home_for_report_with(&mut report, || {
        Err(arf_libr::RError::LibraryNotFound(
            "injected discovery failure".to_string(),
        ))
    });

    assert!(report.r_home.is_none());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "r_discovery.failed");
    assert_eq!(
        report.diagnostics[0].message,
        "Warning: Failed to determine R_HOME from PATH: Failed to discover the R library from PATH"
    );
}

#[test]
fn apply_r_source_does_not_set_r_home_for_path_mode() {
    // Hold the lock to keep concurrent environment writers out while reading R_HOME.
    let _guard = crate::test_utils::lock_env();
    let original = std::env::var_os("R_HOME");
    let report = RSourceResolutionReport {
        status: RSourceStatus::Path,
        r_home: Some(PathBuf::from("/tmp/discovered-r-home")),
        selection_route: RSourceSelectionRoute::Startup,
        override_provider: None,
        file: None,
        key: None,
        requested_version: None,
        resolved_version: None,
        diagnostics: Vec::new(),
        override_state: RSourceOverrideState::NotConfigured,
        explicit_request: None,
    };

    apply_r_source(&report).unwrap();

    assert_eq!(std::env::var_os("R_HOME"), original);
}

#[test]
fn override_state_reflects_how_the_source_was_selected() {
    struct Case {
        name: &'static str,
        overrides: Vec<RSourceOverride>,
        cli_r_home: bool,
        no_r_source_overrides: bool,
        expected_state: RSourceOverrideState,
    }
    let cases = [
        Case {
            name: "no provider file present",
            overrides: vec![RSourceOverride::VersionFile {
                file: ".r-version".into(),
            }],
            cli_r_home: false,
            no_r_source_overrides: false,
            expected_state: RSourceOverrideState::NoMatch,
        },
        Case {
            name: "no providers configured",
            overrides: Vec::new(),
            cli_r_home: false,
            no_r_source_overrides: false,
            expected_state: RSourceOverrideState::NotConfigured,
        },
        Case {
            name: "overrides disabled",
            overrides: vec![RSourceOverride::Pixi],
            cli_r_home: false,
            no_r_source_overrides: true,
            expected_state: RSourceOverrideState::Disabled,
        },
        Case {
            name: "cli r-home shadows a readable override file",
            overrides: vec![RSourceOverride::TomlKey {
                file: "rproject.toml".into(),
                key: "project.r_version".to_string(),
            }],
            cli_r_home: true,
            no_r_source_overrides: false,
            expected_state: RSourceOverrideState::ShadowedByCli,
        },
        Case {
            name: "cli r-home shadows an environment provider",
            overrides: vec![RSourceOverride::Pixi],
            cli_r_home: true,
            no_r_source_overrides: false,
            expected_state: RSourceOverrideState::ShadowedByCli,
        },
        Case {
            name: "cli r-home with overrides disabled",
            overrides: vec![RSourceOverride::Pixi],
            cli_r_home: true,
            no_r_source_overrides: true,
            expected_state: RSourceOverrideState::Disabled,
        },
    ];

    for case in cases {
        let config_dir = tempfile::tempdir().unwrap();
        let cli_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            config_dir.path().join("rproject.toml"),
            "[project]\nr_version = \"4.4\"\n",
        )
        .unwrap();

        let report = setup_r(
            &path_source(config_dir.path()),
            &case.overrides,
            Some(config_dir.path()),
            case.cli_r_home.then_some(cli_dir.path()),
            case.no_r_source_overrides,
        )
        .unwrap();

        assert_eq!(report.override_state, case.expected_state, "{}", case.name);
        assert!(report.diagnostics.is_empty(), "{}", case.name);
        if case.cli_r_home {
            // The explicit R_HOME wins over both the override and startup.r_source.
            assert!(
                matches!(report.status, RSourceStatus::ExplicitPath { .. }),
                "{}",
                case.name
            );
            assert_eq!(
                report.r_home.as_deref(),
                Some(cli_dir.path()),
                "{}",
                case.name
            );
        }
    }
}

#[test]
fn inactive_pixi_workspace_is_reported_and_falls_back() {
    let mut guard = crate::test_utils::lock_env();
    for key in [
        "PIXI_IN_SHELL",
        "PIXI_WORKSPACE_MANIFEST",
        "PIXI_PROJECT_MANIFEST",
        "PIXI_ENVIRONMENT_NAME",
    ] {
        guard.unset(key);
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("pixi.toml"), "[workspace]").unwrap();
    let report = setup_r(
        &path_source(temp.path()),
        &[RSourceOverride::Pixi],
        Some(temp.path()),
        None,
        false,
    )
    .unwrap();

    assert_eq!(report.override_state, RSourceOverrideState::Failed);
    assert_eq!(report.diagnostics.len(), 2);
    assert_eq!(
        report.diagnostics[0].code,
        "r_source_override.pixi_not_active"
    );
    assert_eq!(report.diagnostics[1].code, "r_source_override.fallback");
    assert!(report.diagnostics[0].message.contains("pixi run arf"));
    assert!(
        report.diagnostics[1]
            .message
            .contains("Using startup.r_source: path")
    );
}
