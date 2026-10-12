use super::*;
use nu_ansi_term::Color;
use std::path::PathBuf;

#[test]
fn test_experimental_config_default() {
    let config = ExperimentalConfig::default();
    assert!(!config.history_forget.enabled);
    assert!(config.completion_min_chars.is_none());
    assert!(config.prompt_spinner.frames.is_empty()); // Disabled by default
    assert!(!config.r_completion.fuzzy); // Disabled by default
    assert_eq!(
        config.r_completion.package_functions,
        vec!["library".to_string(), "require".to_string()]
    );
    assert_eq!(
        config.r_completion.r#static.formals.mode,
        StaticFormalsMode::Off
    );
    assert!(
        config
            .r_completion
            .r#static
            .formals
            .exclusions
            .packages
            .is_empty()
    );
    assert!(
        config
            .r_completion
            .r#static
            .formals
            .exclusions
            .functions
            .is_empty()
    );
    assert!(!config.shell_completion.command_names); // Disabled by default
    assert!(config.r_source_overrides.is_empty());
}

#[test]
fn test_parse_spinner_config() {
    let toml_str = r#"
[experimental.prompt_spinner]
frames = "abc"
color = "Red"
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.experimental.prompt_spinner.frames, "abc");
    assert_eq!(config.experimental.prompt_spinner.color, Color::Red);
}

#[test]
fn test_parse_spinner_disabled() {
    let toml_str = r#"
[experimental.prompt_spinner]
frames = ""
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert!(config.experimental.prompt_spinner.frames.is_empty());
}

#[test]
fn test_parse_r_completion() {
    let toml_str = r#"
[experimental.r_completion]
fuzzy = true
package_functions = ["library", "require", "box::use"]
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert!(config.experimental.r_completion.fuzzy);
    assert_eq!(
        config.experimental.r_completion.package_functions,
        vec![
            "library".to_string(),
            "require".to_string(),
            "box::use".to_string()
        ]
    );
}

#[test]
fn test_parse_static_formals_config() {
    let toml_str = r#"
[experimental.r_completion]
fuzzy = true

[experimental.r_completion.static.formals]
mode = "prefer-static"

[experimental.r_completion.static.formals.exclusions]
packages = ["S7", "methods"]
functions = ["rlang::abort"]
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert!(config.experimental.r_completion.fuzzy);
    assert_eq!(
        config.experimental.r_completion.r#static.formals.mode,
        StaticFormalsMode::PreferStatic
    );
    assert_eq!(
        config
            .experimental
            .r_completion
            .r#static
            .formals
            .exclusions
            .packages,
        vec!["S7", "methods"]
    );
    assert_eq!(
        config
            .experimental
            .r_completion
            .r#static
            .formals
            .exclusions
            .functions,
        vec!["rlang::abort"]
    );

    let serialized = toml::to_string(&config).unwrap();
    assert!(serialized.contains("[experimental.r_completion.static.formals]"));
    assert!(!serialized.contains("static_formals"));
    let round_trip: crate::config::Config = toml::from_str(&serialized).unwrap();
    assert_eq!(
        toml::Value::try_from(&config).unwrap(),
        toml::Value::try_from(&round_trip).unwrap()
    );
}

#[test]
fn test_static_formals_defaults_to_off() {
    for source in [
        "",
        "[experimental.r_completion.static]",
        "[experimental.r_completion.static.formals]",
        r#"[experimental.r_completion.static.formals]
mode = "off""#,
        r#"[experimental.r_completion.static.formals.exclusions]
packages = ["stats"]"#,
    ] {
        let config: crate::config::Config = toml::from_str(source).unwrap();
        assert_eq!(
            config.experimental.r_completion.r#static.formals.mode,
            StaticFormalsMode::Off,
            "static formals must remain opt-in: {source}"
        );
    }
}

#[test]
fn test_static_formals_rejects_invalid_modes() {
    for mode in [r#""static-only""#, r#""prefer_static""#, "true"] {
        let source = format!(
            r#"[experimental.r_completion.static.formals]
mode = {mode}"#
        );
        assert!(toml::from_str::<crate::config::Config>(&source).is_err());
    }
}

#[test]
fn test_parse_r_completion_fuzzy_only() {
    // When only fuzzy is specified, package_functions should use defaults
    let toml_str = r#"
[experimental.r_completion]
fuzzy = true
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert!(config.experimental.r_completion.fuzzy);
    assert_eq!(
        config.experimental.r_completion.package_functions,
        vec!["library".to_string(), "require".to_string()]
    );
}

#[test]
fn test_parse_shell_completion() {
    let toml_str = r#"
[experimental.shell_completion]
command_names = true
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert!(config.experimental.shell_completion.command_names);
}

#[test]
fn test_parse_shell_semicolon_shortcut() {
    let toml_str = r#"
[experimental]
shell_semicolon_shortcut = true
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert!(config.experimental.shell_semicolon_shortcut);
}

#[test]
fn test_parse_r_source_overrides() {
    let toml_str = r#"
[experimental]
r_source_overrides = [
  { provider = "toml-key", file = "rproject.toml", key = "project.r_version" },
  { provider = "json-key", file = "renv.lock", key = "R.Version" },
  { provider = "version-file", file = ".r-version" },
  { provider = "pixi" },
]
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();

    assert!(matches!(
        &config.experimental.r_source_overrides[0],
        RSourceOverride::TomlKey { file, key }
            if file == &PathBuf::from("rproject.toml") && key == "project.r_version"
    ));
    assert!(matches!(
        &config.experimental.r_source_overrides[1],
        RSourceOverride::JsonKey { file, key }
            if file == &PathBuf::from("renv.lock") && key == "R.Version"
    ));
    assert!(matches!(
        &config.experimental.r_source_overrides[2],
        RSourceOverride::VersionFile { file } if file == &PathBuf::from(".r-version")
    ));
    assert!(matches!(
        config.experimental.r_source_overrides[3],
        RSourceOverride::Pixi
    ));
}

#[test]
fn test_parse_r_source_override_unknown_provider_fails() {
    let toml_str = r#"
[experimental]
r_source_overrides = [{ provider = "unknown-thing" }]
"#;
    let result: Result<crate::config::Config, _> = toml::from_str(toml_str);
    let error = result.expect_err("unknown R source override providers should fail");
    assert!(error.to_string().contains("unknown variant"));
}

#[test]
fn test_r_source_override_round_trip_uses_provider_tag() {
    let overrides = vec![
        RSourceOverride::VersionFile {
            file: PathBuf::from(".r-version"),
        },
        RSourceOverride::TomlKey {
            file: PathBuf::from("rproject.toml"),
            key: "project.r_version".to_string(),
        },
        RSourceOverride::JsonKey {
            file: PathBuf::from("renv.lock"),
            key: "R.Version".to_string(),
        },
        RSourceOverride::Pixi,
    ];
    let serialized = serde_json::to_string(&overrides).unwrap();
    let deserialized: Vec<RSourceOverride> = serde_json::from_str(&serialized).unwrap();
    let snapshot = serde_json::to_string_pretty(&deserialized).unwrap();
    insta::assert_snapshot!(snapshot, @r###"
    [
      {
        "provider": "version-file",
        "file": ".r-version"
      },
      {
        "provider": "toml-key",
        "file": "rproject.toml",
        "key": "project.r_version"
      },
      {
        "provider": "json-key",
        "file": "renv.lock",
        "key": "R.Version"
      },
      {
        "provider": "pixi"
      }
    ]"###);
}

#[test]
fn test_parse_existing_config_without_r_source_overrides() {
    let toml_str = r#"
[startup]
show_banner = false
"#;
    let config: crate::config::Config = toml::from_str(toml_str).unwrap();
    assert!(!config.startup.show_banner);
    assert!(config.experimental.r_source_overrides.is_empty());
}
