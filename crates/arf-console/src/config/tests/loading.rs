use super::super::*;

#[test]
fn test_mask_home_path_with_home_prefix() {
    // mask_home_path reads HOME indirectly through dirs::home_dir.
    let _guard = crate::test_utils::lock_env();
    if let Some(home) = dirs::home_dir() {
        let test_path = home.join("test").join("file.txt");
        let masked = mask_home_path(&test_path);
        assert!(
            masked.starts_with("~"),
            "Path should start with ~: {}",
            masked
        );
        assert!(
            masked.contains("test"),
            "Path should contain 'test': {}",
            masked
        );
        assert!(
            masked.contains("file.txt"),
            "Path should contain 'file.txt': {}",
            masked
        );
    }
}

#[test]
fn test_mask_home_path_without_home_prefix() {
    // mask_home_path reads HOME indirectly through dirs::home_dir.
    let _guard = crate::test_utils::lock_env();
    let test_path = PathBuf::from("/opt/R/4.5.0");
    let expected = test_path.display().to_string();
    let masked = mask_home_path(&test_path);
    assert_eq!(
        masked, expected,
        "Path without home prefix should be unchanged"
    );
}

#[test]
fn test_mask_home_path_exact_home() {
    // mask_home_path reads HOME indirectly through dirs::home_dir.
    let _guard = crate::test_utils::lock_env();
    if let Some(home) = dirs::home_dir() {
        let masked = mask_home_path(&home);
        // Should be just "~/" or "~\" depending on platform
        assert!(
            masked.starts_with("~"),
            "Home path should start with ~: {}",
            masked
        );
    }
}

#[test]
fn test_load_config_from_path_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "[editor\nauto_match = true").unwrap();

    let result = load_config_from_path(&path);
    assert!(result.is_err(), "Invalid TOML should return Err");

    let err = result.unwrap_err();
    assert!(
        matches!(err, ConfigLoadError::Parse { .. }),
        "Should be a ParseError: {:?}",
        err
    );
    let msg = err.to_string();
    assert!(
        msg.contains("bad.toml"),
        "Error should mention the file path: {}",
        msg
    );
}

#[test]
fn test_load_config_from_path_type_error_includes_location() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid-type.toml");
    std::fs::write(
        &path,
        r#"
[editor]
mode = 42
"#,
    )
    .unwrap();

    let error = load_config_from_path(&path).expect_err("invalid config type should fail");
    let message = error.to_string();

    assert!(
        message.contains("line 3, column 8"),
        "Type error should include source location: {message}"
    );
}

#[test]
fn test_load_config_from_path_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("good.toml");
    std::fs::write(
        &path,
        r#"
[editor]
auto_match = false
"#,
    )
    .unwrap();

    let result = load_config_from_path(&path);
    assert!(result.is_ok(), "Valid TOML should return Ok");
    let config = result.unwrap();
    assert!(!config.editor.auto_match);
}

#[test]
fn test_load_config_from_path_not_found() {
    let path = PathBuf::from("/nonexistent/config.toml");
    let result = load_config_from_path(&path);
    assert!(result.is_ok(), "Missing file should return Ok(default)");
    let config = result.unwrap();
    assert!(config.editor.auto_match, "Should be default config");
}

#[test]
fn test_config_load_error_display() {
    let err = ConfigLoadError::Parse {
        source: toml::from_str::<Config>("[bad\n").unwrap_err(),
        path: PathBuf::from("/home/user/.config/arf/arf.toml"),
    };
    let msg = err.to_string();
    assert!(msg.contains("parse"), "Should mention parse: {}", msg);
    assert!(msg.contains("arf.toml"), "Should mention path: {}", msg);
}

#[cfg(unix)]
#[test]
fn test_load_config_from_path_read_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unreadable.toml");
    std::fs::write(&path, "[editor]\nauto_match = true").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

    let result = load_config_from_path(&path);
    assert!(result.is_err(), "Unreadable file should return Err");

    let err = result.unwrap_err();
    assert!(
        matches!(err, ConfigLoadError::Read { .. }),
        "Should be a ReadError: {:?}",
        err
    );

    // Restore permissions so tempdir cleanup succeeds
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
}
