//! Provider values and file contents must not leak into diagnostics.

use super::*;

#[test]
fn overlong_version_file_value_is_not_disclosed_in_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let marker = "4".repeat(300);
    std::fs::write(temp.path().join("project.r-version"), &marker).unwrap();

    let diagnostics = fallback_diagnostic_text(
        &[RSourceOverride::VersionFile {
            file: "project.r-version".into(),
        }],
        temp.path(),
    );

    assert!(diagnostics.contains("exceeds 256 bytes"));
    assert!(!diagnostics.contains(&marker));
}

#[test]
fn later_version_file_lines_are_not_disclosed_in_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let private_contents = "private trailing contents that must not be disclosed";
    std::fs::write(
        temp.path().join("project.r-version"),
        format!("not-a-version\n{private_contents}\n"),
    )
    .unwrap();

    let diagnostics = fallback_diagnostic_text(
        &[RSourceOverride::VersionFile {
            file: "project.r-version".into(),
        }],
        temp.path(),
    );

    assert!(!diagnostics.contains(private_contents));
}

#[test]
fn invalid_version_markers_are_not_disclosed_for_file_providers() {
    let marker = "SUPER-SECRET-TOKEN-abc123";
    let providers = [
        (
            "project.r-version",
            marker.to_owned(),
            RSourceOverride::VersionFile {
                file: "project.r-version".into(),
            },
        ),
        (
            "rproject.toml",
            format!("[project]\nr_version = \"{marker}\"\n"),
            RSourceOverride::TomlKey {
                file: "rproject.toml".into(),
                key: "project.r_version".to_string(),
            },
        ),
        (
            "renv.lock",
            format!(r#"{{"R":{{"Version":"{marker}"}}}}"#),
            RSourceOverride::JsonKey {
                file: "renv.lock".into(),
                key: "R.Version".to_string(),
            },
        ),
    ];

    for (file, contents, provider) in providers {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(file), contents).unwrap();

        let diagnostics = fallback_diagnostic_text(&[provider], temp.path());

        assert!(!diagnostics.contains(marker), "provider file: {file}");
        assert!(
            diagnostics.contains("could not be parsed"),
            "provider file: {file}"
        );
    }
}

#[test]
fn malformed_files_do_not_disclose_their_contents_in_diagnostics() {
    let toml_marker = "SUPER-SECRET-TOML-CONTENTS";
    let json_marker = "SUPER-SECRET-JSON-CONTENTS";
    let cases = [
        (
            "rproject.toml",
            format!("[project]\nr_version = \"{toml_marker}\n"),
            RSourceOverride::TomlKey {
                file: "rproject.toml".into(),
                key: "project.r_version".to_string(),
            },
            toml_marker,
        ),
        (
            "renv.lock",
            format!(r#"{{"R":{{"Version":"{json_marker}"}}"#),
            RSourceOverride::JsonKey {
                file: "renv.lock".into(),
                key: "R.Version".to_string(),
            },
            json_marker,
        ),
    ];

    for (file, contents, provider, marker) in cases {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(file), contents).unwrap();

        let diagnostics = fallback_diagnostic_text(&[provider], temp.path());

        assert!(
            diagnostics.contains(&format!(
                "Warning: Failed to parse R source override file {file}; trying the next R source override."
            )),
            "provider file: {file}"
        );
        assert!(!diagnostics.contains(marker), "provider file: {file}");
    }
}
