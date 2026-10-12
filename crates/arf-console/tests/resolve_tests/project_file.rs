use super::fixtures::{FakeRigEnvironment, write_uninstalled_project_override_fixture};
use super::normalize::{
    assert_descriptor_path, assert_selected_by_nullable_fields_are_present,
    assert_structured_resolve_error, normalize_descriptor,
};

#[test]
#[cfg(unix)]
fn resolve_uses_arf_config_without_adding_stdout() {
    let temp = write_uninstalled_project_override_fixture();
    let environment = FakeRigEnvironment::new();
    let config_path = temp.path().join("arf.toml");

    let output = environment
        .command()
        .env("ARF_CONFIG", &config_path)
        .args(["r", "resolve", "--no-r-source-overrides"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["source"]["kind"], "configuration_file");
    assert_descriptor_path(
        &value,
        &value["selected_by"]["source"]["path"],
        &config_path,
    );
}

#[test]
#[cfg(unix)]
fn resolve_uninstalled_project_override_falls_back_to_startup_source() {
    let temp = write_uninstalled_project_override_fixture();
    let environment = FakeRigEnvironment::new();

    let fallback_output = environment
        .command()
        .args([
            "r",
            "resolve",
            "--config",
            "arf.toml",
            "--no-r-source-overrides",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(
        fallback_output.status.success(),
        "stderr: {:?}",
        fallback_output.stderr
    );
    let fallback_value: serde_json::Value =
        serde_json::from_slice(&fallback_output.stdout).unwrap();
    assert_eq!(fallback_value["resolved"], true);

    let output = environment
        .command()
        .args(["r", "resolve", "--config", "arf.toml"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["resolved"], true);
    assert_eq!(
        value["target"]["r_home"],
        fallback_value["target"]["r_home"]
    );
    assert_eq!(value["selected_by"]["kind"], "default");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_r_home"].is_null());
    assert!(value["selected_by"]["requested_version"].is_null());
    assert_eq!(value["selected_by"]["source"]["kind"], "configuration_file");
    assert!(value["selected_by"]["source"]["name"].is_null());
    assert_descriptor_path(&value, &value["selected_by"]["source"]["path"], "arf.toml");
    assert_eq!(value["selected_by"]["source"]["format"], "toml");
    assert_eq!(value["selected_by"]["source"]["key"], "startup.r_source");

    let diagnostic = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "r_source_override.version_not_installed")
        .expect("missing uninstalled-version override diagnostic");
    assert_eq!(diagnostic["path"], "rproject.toml");
}

#[test]
#[cfg(unix)]
fn resolve_project_file_version_request_reports_toml_source() {
    let temp = write_uninstalled_project_override_fixture();
    std::fs::write(
        temp.path().join("rproject.toml"),
        r#"[project]
r_version = "4.4.2"
"#,
    )
    .unwrap();
    let environment = FakeRigEnvironment::new();

    let output = environment
        .command()
        .args(["r", "resolve", "--config", "arf.toml"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "version_request");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_r_home"].is_null());
    assert_eq!(value["selected_by"]["requested_version"], "4.4.2");
    assert_eq!(value["selected_by"]["source"]["kind"], "project_file");
    assert!(value["selected_by"]["source"]["name"].is_null());
    assert_descriptor_path(
        &value,
        &value["selected_by"]["source"]["path"],
        "rproject.toml",
    );
    assert_eq!(value["selected_by"]["source"]["format"], "toml");
    assert_eq!(value["selected_by"]["source"]["key"], "project.r_version");
    insta::assert_snapshot!(
        "resolve_project_file_version_request_reports_toml_source",
        normalize_descriptor(&value)
    );
}

#[test]
#[cfg(unix)]
fn resolve_project_file_version_request_reports_json_source() {
    let temp = write_uninstalled_project_override_fixture();
    std::fs::write(
        temp.path().join("arf.toml"),
        r#"[experimental]
r_source_overrides = [
  { provider = "json-key", file = "renv.lock", key = "R.Version" },
]

[startup]
r_source = { path = "fallback-r-home" }
"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join("renv.lock"),
        r#"{"R":{"Version":"4.4.2"}}"#,
    )
    .unwrap();
    let environment = FakeRigEnvironment::new();

    let output = environment
        .command()
        .args(["r", "resolve", "--config", "arf.toml"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "version_request");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_r_home"].is_null());
    assert_eq!(value["selected_by"]["requested_version"], "4.4.2");
    assert_eq!(value["selected_by"]["source"]["kind"], "project_file");
    assert!(value["selected_by"]["source"]["name"].is_null());
    assert_descriptor_path(&value, &value["selected_by"]["source"]["path"], "renv.lock");
    assert_eq!(value["selected_by"]["source"]["format"], "json");
    assert_eq!(value["selected_by"]["source"]["key"], "R.Version");
    insta::assert_snapshot!(
        "resolve_project_file_version_request_reports_json_source",
        normalize_descriptor(&value)
    );
}

#[test]
#[cfg(unix)]
fn resolve_project_file_version_request_reports_text_source() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("arf.toml"),
        r#"[experimental]
r_source_overrides = [{ provider = "version-file", file = ".r-version" }]

[startup]
r_source = { path = "fallback-r-home" }
"#,
    )
    .unwrap();
    std::fs::create_dir(temp.path().join("fallback-r-home")).unwrap();
    std::fs::write(temp.path().join(".r-version"), "4.4.2\n").unwrap();
    let environment = FakeRigEnvironment::new();

    let output = environment
        .command()
        .args(["r", "resolve", "--config", "arf.toml"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "version_request");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_r_home"].is_null());
    assert_eq!(value["selected_by"]["source"]["kind"], "project_file");
    assert!(value["selected_by"]["source"]["name"].is_null());
    assert_descriptor_path(
        &value,
        &value["selected_by"]["source"]["path"],
        ".r-version",
    );
    assert_eq!(value["selected_by"]["source"]["format"], "text");
    assert!(value["selected_by"]["source"]["key"].is_null());
    insta::assert_snapshot!(
        "resolve_project_file_version_request_reports_text_source",
        normalize_descriptor(&value)
    );
}

/// Project configuration is a hint that may fall back, while an explicit
/// user request must fail when the requested R version is not installed.
#[test]
#[cfg(unix)]
fn resolve_project_hint_and_explicit_version_have_different_outcomes() {
    let temp = write_uninstalled_project_override_fixture();
    let environment = FakeRigEnvironment::new();

    let project_hint = environment
        .command()
        .args(["r", "resolve", "--config", "arf.toml"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        project_hint.status.success(),
        "stderr: {:?}",
        project_hint.stderr
    );
    let project_hint_value: serde_json::Value =
        serde_json::from_slice(&project_hint.stdout).unwrap();
    assert_eq!(project_hint_value["resolved"], true);
    assert_eq!(project_hint_value["selected_by"]["kind"], "default");
    assert_selected_by_nullable_fields_are_present(&project_hint_value);
    assert!(project_hint_value["selected_by"]["requested_r_home"].is_null());
    assert!(project_hint_value["selected_by"]["requested_version"].is_null());
    assert_eq!(
        project_hint_value["selected_by"]["source"]["kind"],
        "configuration_file"
    );

    let explicit_request = environment
        .command()
        .args([
            "r",
            "resolve",
            "--config",
            "arf.toml",
            "--with-r-version",
            "3.99.99",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert_structured_resolve_error(&explicit_request, "INVALID_PARAMS");
    assert_eq!(explicit_request.status.code(), Some(2));
}
