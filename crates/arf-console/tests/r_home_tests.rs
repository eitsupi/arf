use std::process::Command;

#[test]
fn plain_r_home_output_is_exactly_the_resolved_path() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r-home", "--r-home"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", temp.path().display())
    );
    assert!(String::from_utf8(output.stderr).unwrap().is_empty());
}

#[test]
fn json_r_home_output_has_resolution_details() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r-home", "--json", "--r-home"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["r_home"], temp.path().display().to_string());
    assert_eq!(value["source"], format!("path ({})", temp.path().display()));
    assert_eq!(value["r_source_override"]["state"], "shadowed_by_cli");
    for field in [
        "provider",
        "file",
        "key",
        "requested_version",
        "resolved_version",
    ] {
        assert!(value["r_source_override"][field].is_null(), "{field}");
    }
    assert!(value["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn r_home_resolution_failure_exits_nonzero() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-r-home");
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r-home", "--r-home"])
        .arg(missing)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("R_HOME path does not exist")
    );
}
