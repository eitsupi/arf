use super::*;

#[test]
fn rig_unavailable_warning_explains_provider_failure() {
    assert_eq!(
        rig_unavailable_warning(&external::rig::RigError::NotInstalled).message,
        "Warning: rig is not installed, so the R source override cannot be resolved.\n         Install rig from https://github.com/r-lib/rig or use \"auto\".\n         Trying the next R source override."
    );
}

#[test]
fn range_not_installed_warning_does_not_suggest_rig_add() {
    let spec = rversion::VersionSpec::parse(">=4.3, <5.0").unwrap();
    let warning = not_installed_warning(
        "toml-key",
        "rproject.toml:project.r_version",
        ">=4.3, <5.0",
        &spec,
        Some(PathBuf::from("rproject.toml")),
    );

    assert_eq!(warning.code, "r_source_override.version_not_installed");
    assert!(
        warning
            .message
            .contains("Install a matching R version with rig")
    );
    assert!(!warning.message.contains("rig add"));
    assert!(warning.message.contains("toml-key"));
    assert!(warning.message.contains("rproject.toml:project.r_version"));
    assert!(!warning.message.contains("Falling back to startup.r_source"));
}

#[test]
fn override_notice_describes_the_applied_override() {
    let info = RSourceOverrideInfo {
        provider: "version-file".to_string(),
        file: ".r-version".into(),
        key: None,
        requested_version: "4.4".to_string(),
        resolved_version: "4.4.2".to_string(),
    };
    let report = RSourceResolutionReport::applied(
        RSourceStatus::Rig {
            version: "4.4.2".to_string(),
            override_info: Some(info.clone()),
        },
        PathBuf::from("/tmp/r-home"),
        info,
        Vec::new(),
    );

    assert_eq!(
        override_notice(&report).as_deref(),
        Some(r#"# R source override: version-file .r-version = "4.4""#)
    );
}
