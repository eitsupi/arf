use super::fixtures::{FakeRigEnvironment, RLessEnvironment};
use super::normalize::{
    assert_selected_by_nullable_fields_are_present, assert_structured_resolve_error,
    normalize_descriptor,
};
use crate::common::write_executable;

#[test]
#[cfg(unix)]
fn resolve_invalid_version_is_invalid_invocation() {
    let environment = FakeRigEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "not-a-version"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
#[cfg(unix)]
fn resolve_valid_but_uninstalled_version_is_invalid_invocation() {
    let environment = FakeRigEnvironment::new();

    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "3.99.99"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("3.99.99")
    );
}

#[test]
#[cfg(not(windows))]
fn resolve_version_without_rig_is_invalid_invocation() {
    let environment = RLessEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "4.5.2"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
#[cfg(unix)]
fn resolve_rig_alias_is_accepted_before_version_spec_validation() {
    let environment = FakeRigEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "my-proj"])
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["target"]["resolved_version"], "4.4.2");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_r_home"].is_null());
    assert_eq!(value["selected_by"]["requested_version"], "my-proj");
    assert_eq!(value["selected_by"]["kind"], "version_request");
    assert_eq!(
        value["selected_by"]["source"]["kind"],
        "command_line_argument"
    );
    assert_eq!(value["selected_by"]["source"]["name"], "--with-r-version");
    insta::assert_snapshot!(
        "resolve_rig_alias_is_accepted_before_version_spec_validation",
        normalize_descriptor(&value)
    );
}

#[test]
#[cfg(unix)]
fn resolve_malformed_rig_output_is_internal_error() {
    let environment = FakeRigEnvironment::new();
    std::fs::write(
        &environment.rig,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$1" = "list" ]; then printf '%s\n' 'not json'; exit 0; fi
exit 1
"#,
    )
    .unwrap();

    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "4.4.2"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INTERNAL_ERROR");
    assert_eq!(output.status.code(), Some(4));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("failed to parse rig output")
    );
}

#[test]
#[cfg(unix)]
fn resolve_failing_rig_is_internal_error() {
    let environment = FakeRigEnvironment::new();
    write_executable(&environment.rig, "#!/bin/sh\nexit 1\n").unwrap();

    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "4.4.2"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INTERNAL_ERROR");
    assert_eq!(output.status.code(), Some(4));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Failed to run rig while checking whether it is available")
    );
}

#[test]
#[cfg(unix)]
fn resolve_r_binary_failure_is_internal_error() {
    let environment = FakeRigEnvironment::new();
    std::fs::write(&environment.r, "#!/bin/sh\nexit 1\n").unwrap();

    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "4.4.2"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INTERNAL_ERROR");
    assert_eq!(output.status.code(), Some(4));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("RHOME failed")
    );
}
