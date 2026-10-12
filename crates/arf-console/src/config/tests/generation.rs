use super::super::*;

#[test]
fn test_generate_default_config() {
    let config_str = generate_default_config();

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
