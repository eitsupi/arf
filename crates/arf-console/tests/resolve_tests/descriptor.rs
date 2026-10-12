#[cfg(unix)]
use super::fixtures::FakeRigEnvironment;
#[cfg(not(windows))]
use super::fixtures::RLessEnvironment;
use super::fixtures::isolated_command;
use super::normalize::{
    assert_descriptor_path, assert_selected_by_nullable_fields_are_present,
    assert_structured_resolve_error, normalize_descriptor,
};

#[test]
fn resolve_found_emits_descriptor_with_target() {
    let temp = tempfile::tempdir().unwrap();
    let output = isolated_command(temp.path())
        .args(["r", "resolve", "--r-home"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["resolved"], true);
    assert_descriptor_path(&value, &value["target"]["r_home"], temp.path());
    assert!(value["target"]["r_binary"].is_null());
    assert!(value["target"]["resolved_version"].is_null());
    assert_eq!(value["selected_by"]["kind"], "r_home");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_version"].is_null());
    assert_eq!(
        value["selected_by"]["source"]["kind"],
        "command_line_argument"
    );
    assert_eq!(value["selected_by"]["source"]["name"], "--r-home");
    assert!(value["selected_by"]["source"]["path"].is_null());
    assert!(value["selected_by"]["source"]["format"].is_null());
    assert!(value["selected_by"]["source"]["key"].is_null());
    assert_descriptor_path(
        &value,
        &value["selected_by"]["requested_r_home"],
        temp.path(),
    );
    assert!(value["diagnostics"].as_array().unwrap().is_empty());
    insta::assert_snapshot!(
        "resolve_found_emits_descriptor_with_target",
        normalize_descriptor(&value)
    );
}

#[test]
fn resolve_relative_r_home_uses_one_absolute_path_representation() {
    let temp = tempfile::tempdir().unwrap();
    let r_home = temp.path().join("rh");
    std::fs::create_dir(&r_home).unwrap();

    let output = isolated_command(temp.path())
        .args(["r", "resolve", "--r-home", "rh"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let target_r_home = value["target"]["r_home"].as_str().unwrap();
    let selected_path = value["selected_by"]["requested_r_home"].as_str().unwrap();
    assert_eq!(target_r_home, selected_path);
    assert!(std::path::Path::new(target_r_home).is_absolute());
    assert_descriptor_path(&value, &value["target"]["r_home"], "rh");
    assert_descriptor_path(&value, &value["selected_by"]["requested_r_home"], "rh");
}

#[test]
#[cfg(unix)]
fn resolve_without_config_reports_built_in_default_source() {
    let temp = tempfile::tempdir().unwrap();
    let environment = FakeRigEnvironment::new();
    let missing_config = temp.path().join("missing-arf.toml");
    let output = environment
        .command()
        .args(["r", "resolve", "--config"])
        .arg(missing_config)
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "default");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_r_home"].is_null());
    assert!(value["selected_by"]["requested_version"].is_null());
    assert_eq!(value["selected_by"]["source"]["kind"], "built_in_default");
    assert!(value["selected_by"]["source"]["name"].is_null());
    assert!(value["selected_by"]["source"]["path"].is_null());
    assert!(value["selected_by"]["source"]["format"].is_null());
    assert!(value["selected_by"]["source"]["key"].is_null());
    insta::assert_snapshot!(
        "resolve_without_config_reports_built_in_default_source",
        normalize_descriptor(&value)
    );
}

#[test]
#[cfg(not(windows))]
fn resolve_not_found_is_successful_false_descriptor() {
    let environment = RLessEnvironment::new();
    let output = environment
        .command()
        .args([
            "r",
            "resolve",
            "--no-r-auto-discovery",
            "--no-r-source-overrides",
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["resolved"], false);
    assert!(value["target"].is_null());
    assert!(value.as_object().unwrap().contains_key("provider"));
    assert!(value["provider"].is_null());
    let diagnostic = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "r_discovery.failed")
        .expect("expected an r_discovery.failed diagnostic");
    // Snapshot the whole message rather than checking a substring, which still
    // passes when the message is wrapped in text describing a different
    // failure, such as a missing path or a PATH search that never ran.
    insta::assert_snapshot!(
        diagnostic["message"].as_str().unwrap(),
        @"R automatic discovery is disabled. Set R_HOME, pass --r-home, or remove --no-r-auto-discovery."
    );
    insta::assert_snapshot!(
        "resolve_not_found_is_successful_false_descriptor",
        normalize_descriptor(&value)
    );
}

#[test]
#[cfg(not(windows))]
fn resolve_explicit_r_home_still_works_with_no_r_auto_discovery() {
    let environment = RLessEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--no-r-auto-discovery", "--r-home"])
        .arg(&environment.fake_r_home)
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["resolved"], true);
    assert_descriptor_path(&value, &value["target"]["r_home"], &environment.fake_r_home);
    assert_eq!(value["provider"], "explicit_path");
    assert_eq!(value["selected_by"]["kind"], "r_home");
}

#[test]
fn resolve_broken_config_falls_back_with_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("broken.toml");
    std::fs::write(&config, "not valid = [toml").unwrap();
    let r_home = temp.path().join("r-home");
    std::fs::create_dir(&r_home).unwrap();

    let output = isolated_command(temp.path())
        .args(["r", "resolve", "--config"])
        .arg(&config)
        .args(["--r-home"])
        .arg(&r_home)
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["resolved"], true);
    assert_descriptor_path(&value, &value["target"]["r_home"], &r_home);
    assert!(
        value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "config.parse_failed")
    );
    insta::assert_snapshot!(
        "resolve_broken_config_falls_back_with_diagnostic",
        normalize_descriptor(&value)
    );
}

#[test]
fn resolve_missing_r_home_returns_invalid_params_exit_code() {
    let temp = tempfile::tempdir().unwrap();
    let missing_r_home = temp.path().join("missing-r-home");
    let output = isolated_command(temp.path())
        .args(["r", "resolve", "--r-home"])
        .arg(missing_r_home)
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}
