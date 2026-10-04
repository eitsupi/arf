use super::super::*;

#[test]
fn test_generate_default_config() {
    let config_str = generate_default_config();

    // Should have Tombi Schema Document Directive on first line
    assert!(config_str.starts_with(
        "#:schema https://raw.githubusercontent.com/eitsupi/arf/main/artifacts/arf.schema.json"
    ));

    // Should be valid TOML
    let parsed: Config =
        toml::from_str(&config_str).expect("Generated config should be valid TOML");

    // Should have default values
    assert!(matches!(
        parsed.startup.r_source,
        RSource::Mode(RSourceMode::Auto)
    ));
    assert!(parsed.startup.show_banner);
    assert_eq!(parsed.editor.mode, EditorMode::Emacs);
    assert_eq!(parsed.experimental.r_help.viewer, HelpViewer::R);
}

#[test]
fn test_generate_default_config_has_new_structure() {
    let config_str = generate_default_config();

    // Should have [startup] section with r_source and show_banner
    assert!(
        config_str.contains("[startup]"),
        "Should have [startup] section"
    );
    assert!(
        config_str.contains("r_source = "),
        "Should have r_source in startup section"
    );
    assert!(
        config_str.contains("show_banner = "),
        "Should have show_banner in startup section"
    );

    // Reprex mode is part of the startup section.
    assert!(
        config_str.contains(r#"reprex = "off""#),
        "Should have reprex mode in startup section"
    );

    // Should have [reprex] section
    assert!(
        config_str.contains("[reprex]"),
        "Should have [reprex] section"
    );

    // Should NOT have old sections
    assert!(
        !config_str.contains("[general]"),
        "Should NOT have [general] section"
    );
    assert!(
        !config_str.contains("[shortcuts]"),
        "Should NOT have [shortcuts] section"
    );
    assert!(
        !config_str.contains("[formatter]"),
        "Should NOT have [formatter] section"
    );

    // Should have other sections
    assert!(
        config_str.contains("[editor]"),
        "Should have [editor] section"
    );
    assert!(
        config_str.contains(r#"mode = "persistent""#),
        "History should default to persistent mode"
    );
    assert!(
        !config_str.contains("disabled"),
        "Deprecated history.disabled must not be serialized"
    );
}
