use super::super::*;
use crokey::KeyCombination;

#[test]
fn test_default_r_help_viewer_is_r() {
    assert_eq!(Config::default().experimental.r_help.viewer, HelpViewer::R);
    for source in ["", "[experimental]", "[experimental.r_help]"] {
        let config: Config = toml::from_str(source).unwrap();
        assert_eq!(config.experimental.r_help.viewer, HelpViewer::R);
    }
}

#[test]
fn test_parse_r_help_viewer_values_and_reject_unknown() {
    for (value, expected) in [("auto", HelpViewer::Auto), ("r", HelpViewer::R)] {
        let source = format!(
            r#"[experimental.r_help]
viewer = "{value}"
"#
        );
        let config: Config = toml::from_str(&source).unwrap();
        assert_eq!(config.experimental.r_help.viewer, expected);
    }

    assert!(
        toml::from_str::<Config>(
            r#"[experimental.r_help]
viewer = "other"
"#
        )
        .is_err()
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
fn reprex_formatter_metadata_describes_backends() {
    for (formatter, display_name, command, install_url, minimum_version) in [
        (
            FormatterBackend::Air,
            "Air",
            "air",
            "https://github.com/posit-dev/air",
            "0.9.0",
        ),
        (
            FormatterBackend::Arity,
            "Arity",
            "arity",
            "https://github.com/jolars/arity",
            "0.18.0",
        ),
    ] {
        assert_eq!(formatter.display_name(), display_name);
        assert_eq!(formatter.command(), command);
        assert_eq!(formatter.install_url(), install_url);
        assert_eq!(formatter.minimum_version(), minimum_version);
        assert_eq!(formatter.to_string(), command);
    }
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
fn test_parse_mode_indicator_positions() {
    for (value, expected) in [
        ("suffix", ModeIndicatorPosition::Suffix),
        ("none", ModeIndicatorPosition::None),
    ] {
        let toml_str = format!("[prompt]\nmode_indicator = \"{value}\"\n");
        let config: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.prompt.mode_indicator, expected);
    }
}
