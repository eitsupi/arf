use super::super::*;

#[test]
fn override_status_display_includes_resolution_metadata() {
    let info = RSourceOverrideInfo {
        provider: "toml-key".to_string(),
        file: "rproject.toml".into(),
        key: Some("project.r_version".to_string()),
        requested_version: "4.4".to_string(),
        resolved_version: "4.4.2".to_string(),
    };
    assert_eq!(
        info.display(),
        r#"toml-key rproject.toml:project.r_version = "4.4""#
    );
    let status = RSourceStatus::Rig {
        version: "4.4.2".to_string(),
        override_info: Some(info),
    };

    assert!(status.switch_available());
    assert_eq!(
        status.display(),
        r#"rig (R 4.4.2; override: toml-key rproject.toml:project.r_version = "4.4")"#
    );
}
