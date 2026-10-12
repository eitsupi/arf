use super::*;
use crate::app::explicit_r_source::RProvider;

#[test]
fn configured_provider_names_match_explicit_provider_names() {
    for provider in <RProvider as clap::ValueEnum>::value_variants() {
        let configured: RSourceOverride =
            serde_json::from_value(serde_json::json!({ "provider": provider.as_str() })).unwrap();
        assert_eq!(ProviderSpec::from(&configured).name, provider.as_str());
    }
}

#[test]
fn version_file_resolution_uses_first_line_only() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("project.r-version"),
        "4.4.2\nprivate trailing contents that must not be read\n",
    )
    .unwrap();

    let result = setup_with_rig_4_4_2(
        &[RSourceOverride::VersionFile {
            file: "project.r-version".into(),
        }],
        temp.path(),
    )
    .unwrap();

    let (info, diagnostics) = expect_applied(result);
    assert_eq!(info.requested_version, "4.4.2");
    assert!(diagnostics.is_empty());
}

#[test]
fn json_key_resolution_uses_nested_string_and_reports_metadata() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("renv.lock"), r#"{"R":{"Version":"4.4"}}"#).unwrap();

    let result = setup_with_rig_4_4_2(
        &[RSourceOverride::JsonKey {
            file: "renv.lock".into(),
            key: "R.Version".to_string(),
        }],
        temp.path(),
    )
    .unwrap();

    let (info, _) = expect_applied(result);
    assert_eq!(info.provider, "json-key");
    assert_eq!(info.file, PathBuf::from("renv.lock"));
    assert_eq!(info.key.as_deref(), Some("R.Version"));
    assert_eq!(info.requested_version, "4.4");
}

#[test]
fn json_key_value_errors_fall_through_to_next_provider() {
    for (name, contents) in [
        ("missing", r#"{"R":{}}"#),
        ("non-string", r#"{"R":{"Version":4.4}}"#),
        ("parse", r#"{"R":{"Version":"4.4"}"#),
    ] {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("renv.lock"), contents).unwrap();
        std::fs::write(temp.path().join("fallback.r-version"), "4.4.2\n").unwrap();

        let result = setup_with_rig_4_4_2(
            &[
                RSourceOverride::JsonKey {
                    file: "renv.lock".into(),
                    key: "R.Version".to_string(),
                },
                RSourceOverride::VersionFile {
                    file: "fallback.r-version".into(),
                },
            ],
            temp.path(),
        )
        .unwrap();

        let (info, diagnostics) = expect_applied(result);
        assert_eq!(info.provider, "version-file", "case: {name}");
        assert_eq!(info.file, Path::new("fallback.r-version"), "case: {name}");
        assert_eq!(diagnostics.len(), 1, "case: {name}");
        if name == "parse" {
            assert!(diagnostics[0].message.contains("Failed to parse"));
        } else {
            assert!(diagnostics[0].message.contains("renv.lock:R.Version"));
        }
    }
}

#[test]
fn invalid_override_filenames_are_skipped_with_diagnostics() {
    struct Provider {
        valid_file: &'static str,
        valid_contents: &'static str,
        configure: fn(PathBuf) -> RSourceOverride,
    }
    let providers = [
        Provider {
            valid_file: "valid.r-version",
            valid_contents: "4.4.2\n",
            configure: |file| RSourceOverride::VersionFile { file },
        },
        Provider {
            valid_file: "valid.toml",
            valid_contents: "[project]\nr_version = \"4.4.2\"\n",
            configure: |file| RSourceOverride::TomlKey {
                file,
                key: "project.r_version".to_string(),
            },
        },
        Provider {
            valid_file: "valid.json",
            valid_contents: r#"{"R":{"Version":"4.4.2"}}"#,
            configure: |file| RSourceOverride::JsonKey {
                file,
                key: "R.Version".to_string(),
            },
        },
    ];
    let invalid_files = ["../x", "sub/x", r"a\b", "/absolute", "C:foo", ".", "..", ""];

    for provider in &providers {
        for invalid_file in invalid_files {
            let temp = tempfile::tempdir().unwrap();
            std::fs::write(
                temp.path().join(provider.valid_file),
                provider.valid_contents,
            )
            .unwrap();

            let result = setup_with_rig_4_4_2(
                &[
                    (provider.configure)(PathBuf::from(invalid_file)),
                    (provider.configure)(PathBuf::from(provider.valid_file)),
                ],
                temp.path(),
            )
            .unwrap();

            let (info, diagnostics) = expect_applied(result);
            assert_eq!(info.file, Path::new(provider.valid_file));
            assert_eq!(diagnostics.len(), 1, "invalid file: {invalid_file:?}");
            assert_eq!(diagnostics[0].code, "r_source_override.value_invalid");
            assert!(
                diagnostics[0]
                    .message
                    .contains("file must be a bare filename")
            );
        }
    }
}
