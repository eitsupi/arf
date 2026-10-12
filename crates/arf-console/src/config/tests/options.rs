use super::super::*;

#[test]
fn test_parse_auto_suggestions_values() {
    for (value, expected) in [
        (r#""none""#, AutoSuggestions::None),
        (r#""all""#, AutoSuggestions::All),
        (r#""cwd""#, AutoSuggestions::Cwd),
        ("true", AutoSuggestions::All),
        ("false", AutoSuggestions::None),
    ] {
        let toml_str = format!("[editor]\nauto_suggestions = {value}\n");
        let config: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.editor.auto_suggestions, expected, "{value}");
    }
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
fn test_parse_auto_suggestions_boolean_strings_rejected() {
    // The strings "true"/"false" are rejected (use the booleans instead).
    for value in [r#""true""#, r#""false""#] {
        let toml_str = format!("[editor]\nauto_suggestions = {value}\n");
        let result: Result<Config, _> = toml::from_str(&toml_str);
        assert!(result.is_err(), "String {value} should be rejected");
    }
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
