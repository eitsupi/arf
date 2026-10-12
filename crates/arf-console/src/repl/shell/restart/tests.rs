use super::*;
use std::ffi::OsStr;

const TEST_STARTUP_ENV_CARRIER: &str = r#"{"version":1,"variables":{}}"#;

#[test]
fn command_failed_rig_switch_error_preserves_reason_without_install_guidance() {
    let error = validate_rig_for_switch_with(|| {
        Err(RigError::CommandFailed("permission denied".to_string()))
    })
    .expect_err("a failed rig command should reject switching");

    assert!(error.contains("permission denied"));
    assert!(!error.contains("Install rig"));
}

#[test]
fn build_restart_command_restores_a_startup_value() {
    let changes = env_changes_for_switch(Some(r"4.0"), |var| {
        (var == "R_LIBS_USER").then(|| OsString::from(r"/user/r/library"))
    });
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    let restored = cmd
        .get_envs()
        .find(|(key, _)| *key == OsStr::new(r"R_LIBS_USER"))
        .map(|(_, value)| value);
    assert_eq!(restored, Some(Some(OsStr::new(r"/user/r/library"))));
}

#[test]
fn build_restart_command_restores_r_libs_from_startup_snapshot() {
    let changes = env_changes_for_switch(Some(r"4.0"), |var| {
        (var == "R_LIBS").then(|| OsString::from(r"/user/r/libs"))
    });
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    let restored = cmd
        .get_envs()
        .find(|(key, _)| *key == OsStr::new(r"R_LIBS"))
        .map(|(_, value)| value);
    assert_eq!(restored, Some(Some(OsStr::new(r"/user/r/libs"))));
}

#[test]
fn build_restart_command_removes_r_libs_absent_from_snapshot() {
    let changes = env_changes_for_switch(Some(r"4.0"), |_| None);
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    assert!(
        cmd.get_envs()
            .any(|(key, value)| key == OsStr::new(r"R_LIBS") && value.is_none())
    );
}

#[test]
fn build_restart_command_always_removes_ld_library_path() {
    let changes = env_changes_for_switch(Some(r"4.0"), |var| {
        (var == "LD_LIBRARY_PATH").then(|| OsString::from(r"/old/r/lib"))
    });
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    assert!(
        cmd.get_envs()
            .any(|(key, value)| key == OsStr::new(r"LD_LIBRARY_PATH") && value.is_none())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn build_restart_command_restores_dyld_library_path_from_startup_snapshot() {
    let changes = env_changes_for_switch(Some(r"4.0"), |var| {
        (var == "DYLD_LIBRARY_PATH").then(|| OsString::from(r"/user/lib"))
    });
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    let restored = cmd
        .get_envs()
        .find(|(key, _)| *key == OsStr::new(r"DYLD_LIBRARY_PATH"))
        .map(|(_, value)| value);
    assert_eq!(restored, Some(Some(OsStr::new(r"/user/lib"))));
}

#[cfg(target_os = "macos")]
#[test]
fn build_restart_command_removes_dyld_library_path_absent_from_snapshot() {
    let changes = env_changes_for_switch(Some(r"4.0"), |_| None);
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    assert!(
        cmd.get_envs()
            .any(|(key, value)| key == OsStr::new(r"DYLD_LIBRARY_PATH") && value.is_none())
    );
}

#[test]
fn build_restart_command_always_removes_r_home_from_snapshot() {
    let changes = env_changes_for_switch(Some(r"4.0"), |var| {
        (var == "R_HOME").then(|| OsString::from(r"/old/r"))
    });
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    assert!(
        cmd.get_envs()
            .any(|(key, value)| key == OsStr::new(r"R_HOME") && value.is_none())
    );
    assert!(!changes.restored.iter().any(|(key, _)| *key == "R_HOME"));
}

#[test]
fn build_restart_command_removes_rig_activation_markers() {
    let changes = env_changes_for_switch(Some(r"4.0"), |var| match var {
        "RVENV" => Some(OsString::from(r"/project/.rvenv")),
        "RVENV_UNSYNCED" => Some(OsString::from("1")),
        _ => None,
    });
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    for marker in ["RVENV", "RVENV_UNSYNCED"] {
        assert!(
            cmd.get_envs()
                .any(|(key, value)| key == OsStr::new(marker) && value.is_none())
        );
        assert!(!changes.restored.iter().any(|(key, _)| *key == marker));
    }
}

#[test]
fn version_none_registers_no_environment_changes() {
    let changes = env_changes_for_switch(None, |_| {
        panic!("restart without a version must not inspect the startup snapshot")
    });
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    assert!(changes.restored.is_empty());
    assert!(changes.removed.is_empty());
    assert!(
        cmd.get_envs()
            .all(|(key, _)| { key != OsStr::new("RVENV") && key != OsStr::new("RVENV_UNSYNCED") })
    );
    assert_eq!(
        cmd.get_envs()
            .find(|(key, _)| *key == OsStr::new(crate::STARTUP_ENV_CARRIER))
            .map(|(_, value)| value),
        Some(Some(OsStr::new(TEST_STARTUP_ENV_CARRIER)))
    );
}

#[test]
fn restart_only_removes_unchanged_arf_supplied_wrapper_paths() {
    let wrapper_values = [
        ("R_DOC_DIR", OsString::from(r"/old/R/doc")),
        ("R_SHARE_DIR", OsString::from(r"/old/R/share")),
        ("R_INCLUDE_DIR", OsString::from(r"/old/R/include")),
    ];
    let removed = arf_supplied_wrapper_paths_to_remove(
        wrapper_values,
        |name| (name == "R_DOC_DIR").then(|| OsString::from(r"/user/selected/doc")),
        |name| match name {
            "R_DOC_DIR" => Some(OsString::from(r"/old/R/doc")),
            "R_SHARE_DIR" => Some(OsString::from(r"/old/R/share")),
            "R_INCLUDE_DIR" => Some(OsString::from(r"/session/selected/include")),
            _ => None,
        },
    );

    assert_eq!(removed, ["R_SHARE_DIR"]);
}

#[test]
fn build_restart_command_sets_carrier_for_switch() {
    let changes = env_changes_for_switch(Some(r"4.0"), |_| None);
    let cmd = build_restart_command(
        std::path::Path::new(r"arf"),
        &[],
        &changes,
        TEST_STARTUP_ENV_CARRIER,
    );

    assert_eq!(
        cmd.get_envs()
            .find(|(key, _)| *key == OsStr::new(crate::STARTUP_ENV_CARRIER))
            .map(|(_, value)| value),
        Some(Some(OsStr::new(TEST_STARTUP_ENV_CARRIER)))
    );
}

#[test]
fn switching_replaces_explicit_r_sources_before_user_arguments() {
    let args = vec![
        OsString::from("--quiet"),
        OsString::from("--with-r-version"),
        OsString::from("4.3"),
        OsString::from("--r-provider=rig-project"),
        OsString::from("--r-provider"),
        OsString::from("pixi"),
        OsString::from("--with-r-version=4.4"),
        OsString::from("--"),
        OsString::from("--r-provider"),
        OsString::from("script-argument"),
    ];

    assert_eq!(
        args_with_r_version(args, "4.5"),
        vec![
            OsString::from("--quiet"),
            OsString::from("--with-r-version"),
            OsString::from("4.5"),
            OsString::from("--"),
            OsString::from("--r-provider"),
            OsString::from("script-argument"),
        ]
    );
}

#[cfg(unix)]
#[test]
fn switching_preserves_non_utf8_arguments_after_separator() {
    use std::os::unix::ffi::OsStringExt;

    let user_arg = OsString::from_vec(vec![b'R', b'-', 0xff]);
    let args = vec![OsString::from("--"), user_arg.clone()];

    let switched = args_with_r_version(args, "4.5");

    assert_eq!(
        switched,
        vec![
            OsString::from("--with-r-version"),
            OsString::from("4.5"),
            OsString::from("--"),
            user_arg,
        ]
    );
}
