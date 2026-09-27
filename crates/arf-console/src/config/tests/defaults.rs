use super::super::*;
use crokey::KeyCombination;

#[test]
fn test_default_config() {
    let config = Config::default();
    assert!(
        config.editor.auto_match,
        "auto_match should be enabled by default"
    );
    assert_eq!(config.editor.mode, EditorMode::Emacs);
    assert!(matches!(
        config.startup.r_source,
        RSource::Mode(RSourceMode::Auto)
    ));
    assert!(config.startup.show_banner);
    assert_eq!(config.reprex.formatter, ReprexFormatter::Auto);
}

#[test]
fn test_default_r_auto_width() {
    let config = Config::default();
    assert!(
        config.r.auto_width,
        "auto_width should be enabled by default"
    );
}

#[test]
fn test_parse_r_auto_width_disabled() {
    let toml_str = r#"
[r]
auto_width = false
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(!config.r.auto_width);
}

#[test]
fn test_parse_r_auto_width_default_when_omitted() {
    let toml_str = r#"
[editor]
mode = "vi"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(
        config.r.auto_width,
        "auto_width should default to true when [r] section is omitted"
    );
}

#[test]
fn test_parse_config_with_auto_match_enabled() {
    let toml_str = r#"
[editor]
auto_match = true
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(config.editor.auto_match);
}

#[test]
fn test_parse_config_with_auto_match_disabled() {
    let toml_str = r#"
[editor]
auto_match = false
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(!config.editor.auto_match);
}

#[test]
fn test_parse_startup_section_config() {
    let toml_str = r##"
[startup]
r_source = "rig"
show_banner = false
reprex = "on"

[editor]
mode = "vi"
auto_match = false

[prompt]
format = "R> "
continuation = ".. "

[completion]
enabled = true
timeout_ms = 100

[reprex]
comment = "# "
formatter = "air"
"##;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(matches!(
        config.startup.r_source,
        RSource::Mode(RSourceMode::Rig)
    ));
    assert!(!config.startup.show_banner);
    assert_eq!(config.editor.mode, EditorMode::Vi);
    assert!(!config.editor.auto_match);
    assert_eq!(config.prompt.format, "R> ");
    assert_eq!(config.startup.reprex, ReprexMode::On);
    assert_eq!(config.reprex.comment, "# ");
    assert_eq!(config.reprex.formatter, ReprexFormatter::Air);
}

#[test]
fn reprex_formatter_accepts_supported_selectors() {
    for (source, expected) in [
        ("[reprex]\nformatter = \"auto\"", ReprexFormatter::Auto),
        ("[reprex]\nformatter = \"air\"", ReprexFormatter::Air),
        ("[reprex]\nformatter = \"arity\"", ReprexFormatter::Arity),
    ] {
        let config: Config = toml::from_str(source).unwrap();
        assert_eq!(config.reprex.formatter, expected);
    }
}

#[test]
fn reprex_formatter_rejects_unknown_backends() {
    let source = "[reprex]\nformatter = \"unknown\"";
    assert!(toml::from_str::<Config>(source).is_err());
}

#[test]
fn reprex_formatter_metadata_describes_air_backend() {
    let formatter = FormatterBackend::Air;
    assert_eq!(formatter.display_name(), "Air");
    assert_eq!(formatter.command(), "air");
    assert_eq!(formatter.install_url(), "https://github.com/posit-dev/air");
    assert_eq!(formatter.minimum_version(), "0.9.0");
    assert_eq!(formatter.to_string(), "air");
}

#[test]
fn reprex_formatter_metadata_describes_arity_backend() {
    let formatter = FormatterBackend::Arity;
    assert_eq!(formatter.display_name(), "Arity");
    assert_eq!(formatter.command(), "arity");
    assert_eq!(formatter.install_url(), "https://github.com/jolars/arity");
    assert_eq!(formatter.minimum_version(), "0.18.0");
    assert_eq!(formatter.to_string(), "arity");
}

#[test]
fn removed_reprex_configuration_keys_are_rejected_explicitly() {
    for source in [
        r#"[startup.mode]
reprex = true"#,
        r##"[mode.reprex]
comment = "#> ""##,
        r#"[reprex]
enabled = true"#,
    ] {
        let document: toml::Value = toml::from_str(source).unwrap();
        let message = removed_reprex_keys(&document).expect("removed key should be found");
        assert!(!message.is_empty());
    }
}

#[test]
fn removed_reprex_configuration_keys_are_all_reported() {
    let source = r##"
[startup.mode]
reprex = true

[mode.reprex]
comment = "#> "

[reprex]
enabled = true
autoformat = true

[prompt.indicators]
autoformat = true
"##;
    let document: toml::Value = toml::from_str(source).unwrap();
    let message = removed_reprex_keys(&document).expect("removed keys should be found");

    insta::assert_snapshot!(message, @r###"
[startup.mode] was removed; use [startup] reprex = "off"|"on"|"format"
  [mode.reprex] was removed; use [reprex]
  reprex.enabled was removed; use startup.reprex
  reprex.autoformat was removed; use startup.reprex
  prompt.indicators.autoformat was removed; use prompt.indicators.reprex_format
"###);
}

#[test]
fn removed_reprex_configuration_keys_are_rejected_by_file_loader() {
    let cases = [
        r#"[startup.mode]
reprex = true"#,
        r##"[mode.reprex]
comment = "#> ""##,
        r#"[reprex]
enabled = true"#,
        r#"[reprex]
autoformat = true"#,
    ];

    for source in cases {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), source).unwrap();
        let error = load_config_from_path(file.path()).expect_err(source);
        assert!(
            matches!(error, ConfigLoadError::Validation { .. }),
            "removed key must fail validation through the loader: {source}"
        );
    }
}

#[test]
fn test_parse_new_key_map_config() {
    let toml_str = r#"
[editor]
mode = "emacs"

[editor.key_map]
"alt-hyphen" = " <- "
"ctrl-shift-m" = " |> "
"alt-=" = " == "
"#;
    let config: Config = toml::from_str(toml_str).unwrap();

    let alt_hyphen: KeyCombination = "alt-hyphen".parse().unwrap();
    let ctrl_shift_m: KeyCombination = "ctrl-shift-m".parse().unwrap();
    let alt_eq: KeyCombination = "alt-=".parse().unwrap();

    assert_eq!(
        config.editor.key_map.get(&alt_hyphen),
        Some(&" <- ".to_string())
    );
    assert_eq!(
        config.editor.key_map.get(&ctrl_shift_m),
        Some(&" |> ".to_string())
    );
    assert_eq!(
        config.editor.key_map.get(&alt_eq),
        Some(&" == ".to_string())
    );
}

#[test]
fn test_default_key_map() {
    let config = Config::default();

    let alt_hyphen: KeyCombination = "alt-hyphen".parse().unwrap();
    let alt_p: KeyCombination = "alt-p".parse().unwrap();

    assert_eq!(
        config.editor.key_map.get(&alt_hyphen),
        Some(&" <- ".to_string())
    );
    assert_eq!(config.editor.key_map.get(&alt_p), Some(&" |> ".to_string()));
}

#[test]
fn test_default_mode_indicator() {
    let config = Config::default();
    assert_eq!(config.prompt.mode_indicator, ModeIndicatorPosition::Prefix);
    assert_eq!(config.prompt.indicators.reprex, "[reprex] ");
    assert_eq!(config.prompt.indicators.reprex_format, "[format] ");
}

#[test]
fn test_parse_mode_indicator_suffix() {
    let toml_str = r#"
[prompt]
mode_indicator = "suffix"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.prompt.mode_indicator, ModeIndicatorPosition::Suffix);
}

#[test]
fn test_parse_mode_indicator_none() {
    let toml_str = r#"
[prompt]
mode_indicator = "none"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.prompt.mode_indicator, ModeIndicatorPosition::None);
}
