use super::super::*;

#[test]
fn test_auto_suggestions_default_is_all() {
    let config = Config::default();
    assert_eq!(config.editor.auto_suggestions, AutoSuggestions::All);
}

#[test]
fn test_parse_auto_suggestions_none_string() {
    let toml_str = r#"
[editor]
auto_suggestions = "none"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.editor.auto_suggestions, AutoSuggestions::None);
}

#[test]
fn test_parse_auto_suggestions_all_string() {
    let toml_str = r#"
[editor]
auto_suggestions = "all"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.editor.auto_suggestions, AutoSuggestions::All);
}

#[test]
fn test_parse_auto_suggestions_cwd_string() {
    let toml_str = r#"
[editor]
auto_suggestions = "cwd"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.editor.auto_suggestions, AutoSuggestions::Cwd);
}

#[test]
fn test_parse_auto_suggestions_bool_true() {
    let toml_str = r#"
[editor]
auto_suggestions = true
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.editor.auto_suggestions, AutoSuggestions::All);
}

#[test]
fn test_parse_auto_suggestions_bool_false() {
    let toml_str = r#"
[editor]
auto_suggestions = false
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.editor.auto_suggestions, AutoSuggestions::None);
}

#[test]
fn test_parse_auto_suggestions_invalid_string() {
    let toml_str = r#"
[editor]
auto_suggestions = "invalid"
"#;
    let result: Result<Config, _> = toml::from_str(toml_str);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("unknown variant"),
        "Error should mention unknown variant: {}",
        err
    );
}

#[test]
fn test_parse_auto_suggestions_string_true_rejected() {
    // String "true" should be rejected (use boolean true instead)
    let toml_str = r#"
[editor]
auto_suggestions = "true"
"#;
    let result: Result<Config, _> = toml::from_str(toml_str);
    assert!(result.is_err(), "String 'true' should be rejected");
}

#[test]
fn test_parse_auto_suggestions_string_false_rejected() {
    // String "false" should be rejected (use boolean false instead)
    let toml_str = r#"
[editor]
auto_suggestions = "false"
"#;
    let result: Result<Config, _> = toml::from_str(toml_str);
    assert!(result.is_err(), "String 'false' should be rejected");
}

#[test]
fn test_parse_r_source_auto() {
    let toml_str = r#"
[startup]
r_source = "auto"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(matches!(
        config.startup.r_source,
        RSource::Mode(RSourceMode::Auto)
    ));
}

#[test]
fn test_parse_r_source_rig() {
    let toml_str = r#"
[startup]
r_source = "rig"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(matches!(
        config.startup.r_source,
        RSource::Mode(RSourceMode::Rig)
    ));
}

#[test]
fn test_parse_r_source_path() {
    let toml_str = r#"
[startup]
r_source = { path = "/opt/R/4.5.2" }
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    match &config.startup.r_source {
        RSource::Path { path } => {
            assert_eq!(path, &PathBuf::from("/opt/R/4.5.2"));
        }
        _ => panic!("Expected RSource::Path"),
    }
}

#[test]
fn test_parse_r_source_default_when_omitted() {
    let toml_str = r#"
[startup]
show_banner = false
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(matches!(
        config.startup.r_source,
        RSource::Mode(RSourceMode::Auto)
    ));
}

#[test]
fn test_parse_ipc_eval_allowed_functions() {
    let config: Config = toml::from_str(
        r#"
[ipc.eval]
allowed_functions = ["mean", "stats::median", "+"]
"#,
    )
    .unwrap();
    assert_eq!(
        config.ipc.eval.allowed_functions,
        ["mean", "stats::median", "+"]
    );
    assert!(Config::default().ipc.eval.allowed_functions.is_empty());
}
