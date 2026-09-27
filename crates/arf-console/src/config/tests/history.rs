use super::super::*;

#[test]
fn test_parse_history_mode_volatile() {
    let toml_str = r#"
[history]
mode = "volatile"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(matches!(config.history.mode, HistoryMode::Volatile));
}

#[test]
fn test_parse_history_mode_with_directory_object() {
    let config: Config =
        toml::from_str("[history]\nmode = { dir = \"/custom/history\" }\n").unwrap();
    assert!(matches!(
        config.history.mode,
        HistoryMode::Persistent { dir: Some(ref dir) }
            if dir == std::path::Path::new("/custom/history")
    ));

    let serialized = toml::to_string(&config).unwrap();
    assert!(
        serialized.contains("[history.mode]") && serialized.contains(r#"dir = "/custom/history""#),
        "serialized history config used an unexpected TOML shape: {serialized}"
    );
    assert!(!serialized.contains("[history]\ndir = "));
}

#[test]
fn test_history_mode_object_requires_dir_and_rejects_unknown_fields() {
    assert!(toml::from_str::<Config>("[history]\nmode = {}\n").is_err());
    assert!(
        toml::from_str::<Config>("[history]\nmode = { dir = \"/tmp\", extra = true }\n").is_err()
    );
}

#[test]
fn test_history_mode_rejects_legacy_dir_when_explicit() {
    let result =
        toml::from_str::<Config>("[history]\nmode = \"persistent\"\ndir = \"/tmp/history\"\n");
    assert!(result.is_err());
}

#[test]
fn test_legacy_history_disabled_migrates_with_warning() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("arf.toml");
    fs::write(&path, "[history]\ndisabled = true\n").unwrap();
    let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
    assert!(matches!(config.history.mode, HistoryMode::Volatile));
    assert!(config.history_migration_warning.is_some());
    assert!(provenance.unwrap().history_migration_warning.is_some());
}

#[test]
fn test_legacy_history_disabled_false_preserves_directory() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("arf.toml");
    fs::write(
        &path,
        "[history]\ndisabled = false\ndir = \"/tmp/arf-history\"\n",
    )
    .unwrap();
    let (loaded, provenance) = load_config_from_path_with_provenance(&path).unwrap();
    assert!(
        provenance
            .unwrap()
            .history_migration_warning
            .unwrap()
            .contains("persistent")
    );
    assert!(matches!(
        loaded.history.mode,
        HistoryMode::Persistent { .. }
    ));

    let config: Config =
        toml::from_str("[history]\ndisabled = false\ndir = \"/tmp/arf-history\"\n").unwrap();
    assert!(matches!(
        config.history.mode,
        HistoryMode::Persistent { dir: Some(ref dir) } if dir == std::path::Path::new("/tmp/arf-history")
    ));
}

#[test]
fn test_legacy_history_dir_without_disabled_migrates_to_persistent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("arf.toml");
    fs::write(&path, "[history]\ndir = \"/tmp/arf-history\"\n").unwrap();
    let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
    assert!(matches!(
        config.history.mode,
        HistoryMode::Persistent { dir: Some(ref dir) }
            if dir == std::path::Path::new("/tmp/arf-history")
    ));
    assert!(
        provenance
            .unwrap()
            .history_migration_warning
            .unwrap()
            .contains("deprecated")
    );
}

#[test]
fn test_history_mode_wins_over_legacy_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("arf.toml");
    fs::write(&path, "[history]\nmode = \"volatile\"\ndisabled = false\n").unwrap();
    let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
    assert!(
        provenance
            .unwrap()
            .history_migration_warning
            .unwrap()
            .contains("ignored")
    );
    assert!(matches!(config.history.mode, HistoryMode::Volatile));
}

#[test]
fn test_history_mode_object_wins_over_legacy_disabled() {
    for disabled in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("arf.toml");
        fs::write(
            &path,
            format!(
                "[history]\nmode = {{ dir = \"/tmp/payload-history\" }}\ndisabled = {disabled}\n"
            ),
        )
        .unwrap();
        let (config, provenance) = load_config_from_path_with_provenance(&path).unwrap();
        assert!(matches!(
            config.history.mode,
            HistoryMode::Persistent { dir: Some(ref dir) }
                if dir == std::path::Path::new("/tmp/payload-history")
        ));
        assert!(
            provenance
                .unwrap()
                .history_migration_warning
                .unwrap()
                .contains("ignored")
        );
    }
}

#[test]
fn test_history_mode_wrong_type_is_parse_error() {
    let result = toml::from_str::<Config>("[history]\nmode = 1\n");
    assert!(result.is_err());
    let result = toml::from_str::<Config>("[history]\ndisabled = \"true\"\n");
    assert!(result.is_err());
}

#[test]
fn history_mode_overrides_keep_cli_env_config_default_precedence() {
    let configured_dir = PathBuf::from("/config/history");
    let configured = HistoryMode::Persistent {
        dir: Some(configured_dir.clone()),
    };
    let cli_dir = PathBuf::from("/cli-or-env/history");

    assert_eq!(
        history_mode_with_overrides(&configured, Some(&cli_dir), false),
        HistoryMode::Persistent { dir: Some(cli_dir) }
    );
    assert_eq!(
        history_mode_with_overrides(&configured, None, false),
        configured
    );
    assert_eq!(
        history_mode_with_overrides(&HistoryMode::Persistent { dir: None }, None, true),
        HistoryMode::Volatile
    );
}

#[test]
fn test_default_history_forget_config() {
    let config = Config::default();
    assert!(!config.experimental.history_forget.enabled);
    assert_eq!(config.experimental.history_forget.delay, 2);
    assert!(!config.experimental.history_forget.on_exit_only);
}

#[test]
fn test_parse_history_forget_config() {
    let toml_str = r#"
[experimental.history_forget]
enabled = true
delay = 5
on_exit_only = true
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(config.experimental.history_forget.enabled);
    assert_eq!(config.experimental.history_forget.delay, 5);
    assert!(config.experimental.history_forget.on_exit_only);
}
