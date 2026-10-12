use super::*;
use files::MAX_VERSION_FILE_VALUE_LENGTH;
use semver::Version;
use std::io;
use std::path::Path;

fn versions(values: &[&str]) -> Vec<Version> {
    values
        .iter()
        .map(|value| Version::parse(value).unwrap())
        .collect()
}

#[test]
fn patch_precision_does_not_match_a_different_patch() {
    let installed = versions(&["4.4.2"]);
    let spec = VersionSpec::parse("4.4.1").unwrap();

    assert_eq!(resolve_version(&spec, &installed), None);
}

#[test]
fn minor_precision_matches_all_requested_patches_only() {
    let installed = versions(&["4.4.0", "4.4.1", "4.4.2", "4.5.0"]);
    let spec = VersionSpec::parse("4.4").unwrap();

    let selected = resolve_version(&spec, &installed).unwrap();
    assert_eq!(selected, &Version::parse("4.4.2").unwrap());
    assert!(!spec.matches(&Version::parse("4.5.0").unwrap()));
    assert!(spec.matches(&Version::parse("4.4.0").unwrap()));
    assert!(spec.matches(&Version::parse("4.4.1").unwrap()));
    assert!(spec.matches(&Version::parse("4.4.2").unwrap()));
}

#[test]
fn patch_precision_does_not_cross_a_digit_boundary() {
    let installed = versions(&["4.4.10"]);
    let spec = VersionSpec::parse("4.4.1").unwrap();

    assert_eq!(resolve_version(&spec, &installed), None);
}

#[test]
fn major_precision_matches_the_whole_major_series() {
    let installed = versions(&["3.6.3", "4.0.0", "4.4.2", "4.10.0"]);
    let spec = VersionSpec::parse("4").unwrap();

    assert_eq!(
        resolve_version(&spec, &installed).unwrap().to_string(),
        "4.10.0"
    );
    assert!(!spec.matches(&Version::parse("3.6.3").unwrap()));
}

#[test]
fn highest_matching_version_is_selected() {
    let installed = versions(&["4.4.1", "4.4.10", "4.4.2"]);
    let spec = VersionSpec::parse("4.4").unwrap();

    assert_eq!(
        resolve_version(&spec, &installed).unwrap().to_string(),
        "4.4.10"
    );
}

#[test]
fn version_ranges_are_parsed_and_applied() {
    let installed = versions(&["4.3.9", "4.4.0", "4.4.5", "4.5.0", "5.0.0"]);

    let caret = VersionSpec::parse("^4.4").unwrap();
    assert_eq!(
        resolve_version(&caret, &installed).unwrap().to_string(),
        "4.5.0"
    );
    assert!(!caret.matches(&Version::parse("4.3.9").unwrap()));
    assert!(!caret.matches(&Version::parse("5.0.0").unwrap()));

    let comparison = VersionSpec::parse(">=4.3, <5.0").unwrap();
    assert_eq!(
        resolve_version(&comparison, &installed)
            .unwrap()
            .to_string(),
        "4.5.0"
    );

    let tilde = VersionSpec::parse("~4.4").unwrap();
    assert_eq!(
        resolve_version(&tilde, &installed).unwrap().to_string(),
        "4.4.5"
    );
    assert!(!tilde.matches(&Version::parse("4.5.0").unwrap()));
}

#[test]
fn supported_r_version_specifications_are_accepted() {
    for specification in ["4.4.2", "4.4", "4", "^4.4", ">=4.3, <5.0", "~4.4", "*"] {
        assert!(
            VersionSpec::parse(specification).is_ok(),
            "expected {specification:?} to be accepted"
        );
    }
}

#[test]
fn named_specs_use_a_dedicated_variant() {
    assert_eq!(
        VersionSpec::parse("devel").unwrap(),
        VersionSpec::Named("devel".into())
    );
    assert_eq!(
        VersionSpec::parse("release").unwrap(),
        VersionSpec::Named("release".into())
    );
}

#[test]
fn invalid_specs_are_rejected() {
    assert!(matches!(
        VersionSpec::parse(""),
        Err(VersionSpecParseError::Empty)
    ));
    assert!(VersionSpec::parse("r-4.4.1").is_err());
}

#[test]
fn numeric_specs_longer_than_a_version_are_rejected() {
    assert!(matches!(
        VersionSpec::parse("4.4.1.0"),
        Err(VersionSpecParseError::Invalid)
    ));
}

#[test]
fn prerelease_and_build_metadata_specs_are_rejected() {
    for specification in [
        "4.3.0-SUPER-SECRET-TOKEN",
        ">=4.3.0-alpha",
        "=4.4.2-x",
        ">=4.3.0+SUPERSECRET",
        "^4.4.0+build",
    ] {
        assert!(matches!(
            VersionSpec::parse(specification),
            Err(VersionSpecParseError::Invalid)
        ));
    }
}

#[test]
fn version_file_uses_first_non_empty_line() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "\n  4.4.2\r\nprivate trailing contents\n").unwrap();

    assert_eq!(read_version_file(file.path()).unwrap(), "4.4.2");
}

#[test]
fn version_file_rejects_overlong_first_non_empty_line() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        file.path(),
        format!("{}\n", "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH + 1)),
    )
    .unwrap();

    let error = read_version_file(file.path()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "version file value exceeds 256 bytes");
}

#[test]
fn version_file_accepts_value_one_byte_under_limit_with_unix_and_windows_line_endings() {
    for line_ending in ["\n", "\r\n"] {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            format!(
                "{}{}",
                "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH - 1),
                line_ending
            ),
        )
        .unwrap();

        assert_eq!(
            read_version_file(file.path()).unwrap(),
            "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH - 1)
        );
    }
}

#[test]
fn version_file_accepts_value_at_limit_with_unix_and_windows_line_endings() {
    for line_ending in ["\n", "\r\n"] {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            format!(
                "{}{}",
                "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH),
                line_ending
            ),
        )
        .unwrap();

        assert_eq!(
            read_version_file(file.path()).unwrap(),
            "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH)
        );
    }
}

#[test]
fn version_file_rejects_value_one_byte_over_limit_with_unix_and_windows_line_endings() {
    for line_ending in ["\n", "\r\n"] {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            format!(
                "{}{}",
                "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH + 1),
                line_ending
            ),
        )
        .unwrap();

        let error = read_version_file(file.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "version file value exceeds 256 bytes");
    }
}

#[test]
fn version_file_accepts_value_at_limit_at_end_of_file_without_trailing_newline() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let value = "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH);
    std::fs::write(file.path(), &value).unwrap();

    assert_eq!(read_version_file(file.path()).unwrap(), value);
}

#[test]
fn version_file_rejects_an_enormous_single_line() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), format!("{}\n", "4".repeat(1024 * 1024))).unwrap();

    let error = read_version_file(file.path()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn version_file_rejects_an_enormous_whitespace_only_line() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), format!("{}\n", " ".repeat(1024 * 1024))).unwrap();

    let error = read_version_file(file.path()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn toml_key_reads_nested_string() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "[project]\nr_version = \"4.4\"\n").unwrap();

    assert_eq!(
        read_toml_key(file.path(), "project.r_version").unwrap(),
        "4.4"
    );
}

#[test]
fn toml_key_reports_missing_key() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "[project]\nname = \"arf\"\n").unwrap();

    assert!(matches!(
        read_toml_key(file.path(), "project.r_version"),
        Err(TomlKeyError::MissingKey(_))
    ));
}

#[test]
fn toml_key_reports_parse_errors() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "[project\n").unwrap();

    assert!(matches!(
        read_toml_key(file.path(), "project.r_version"),
        Err(TomlKeyError::Parse(_))
    ));
}

#[test]
fn toml_key_does_not_search_parent_directories() {
    let _cwd_lock = crate::test_utils::lock_cwd();
    let parent = tempfile::tempdir().unwrap();
    let child = parent.path().join("child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(
        parent.path().join("rproject.toml"),
        "[project]\nr_version = \"4.4\"\n",
    )
    .unwrap();
    std::env::set_current_dir(&child).unwrap();

    let result = read_toml_key(Path::new("rproject.toml"), "project.r_version");

    assert!(matches!(
        result,
        Err(TomlKeyError::Read(error)) if error.kind() == io::ErrorKind::NotFound
    ));
}

#[test]
fn json_key_reads_nested_string() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), r#"{"R":{"Version":"4.4"}}"#).unwrap();

    assert_eq!(read_json_key(file.path(), "R.Version").unwrap(), "4.4");
}

#[test]
fn json_key_reports_missing_key() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), r#"{"R":{"Platform":"x86_64"}}"#).unwrap();

    assert!(matches!(
        read_json_key(file.path(), "R.Version"),
        Err(JsonKeyError::MissingKey(_))
    ));
}

#[test]
fn json_key_reports_non_string_value() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), r#"{"R":{"Version":4.4}}"#).unwrap();

    assert!(matches!(
        read_json_key(file.path(), "R.Version"),
        Err(JsonKeyError::NotString(_))
    ));
}

#[test]
fn json_key_reports_parse_errors_without_disclosing_contents() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let contents = r#"{"private":"SUPER-SECRET-JSON-CONTENTS""#;
    std::fs::write(file.path(), contents).unwrap();

    let error = read_json_key(file.path(), "R.Version").unwrap_err();
    assert!(matches!(&error, JsonKeyError::Parse(_)));
    assert_eq!(error.to_string(), "failed to parse JSON");
    assert!(!error.to_string().contains("SUPER-SECRET-JSON-CONTENTS"));
}

#[test]
fn json_key_reports_read_errors_and_not_found() {
    let path = Path::new("file-that-does-not-exist.json");

    let error = read_json_key(path, "R.Version").unwrap_err();
    assert!(error.is_not_found());
    assert!(matches!(error, JsonKeyError::Read(_)));
}

#[test]
fn json_key_empty_component_is_missing_key() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), r#"{"R":{"Version":"4.4"}}"#).unwrap();

    assert!(matches!(
        read_json_key(file.path(), "R..Version"),
        Err(JsonKeyError::MissingKey(_))
    ));
}

#[test]
fn json_key_does_not_support_array_indexes() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), r#"{"R":[{"Version":"4.4"}]}"#).unwrap();

    assert!(matches!(
        read_json_key(file.path(), "R.0.Version"),
        Err(JsonKeyError::MissingKey(_))
    ));
}
