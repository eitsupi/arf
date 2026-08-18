use super::super::*;
use super::support::*;

#[test]
fn parses_commands_and_preserves_meaningful_whitespace() {
    let file = text_fixture(
        r#"library(dplyr)
  indented()

  
print("quoted")
"#,
    );

    let parsed = parse_r_history(file.path()).unwrap();
    let expected = [
        r("library(dplyr)"),
        r("  indented()"),
        r(r#"print("quoted")"#),
    ];
    assert_eq!(parsed.entries, expected);
    assert!(parsed.warnings.is_empty());
}

#[test]
fn parses_empty_and_file_not_found() {
    let empty = text_fixture("");
    assert!(parse_r_history(empty.path()).unwrap().entries.is_empty());

    let missing = tempfile::tempdir().unwrap().path().join("missing.Rhistory");
    assert!(parse_r_history(&missing).is_err());
}

#[test]
#[serial_test::serial]
fn default_r_history_path_has_deterministic_override_and_fallback() {
    let override_path = "/tmp/arf-test-history";
    unsafe { std::env::set_var("R_HISTFILE", override_path) };
    assert_eq!(
        default_r_history_path(),
        std::path::PathBuf::from(override_path)
    );
    unsafe { std::env::remove_var("R_HISTFILE") };
    assert_eq!(
        default_r_history_path(),
        std::path::PathBuf::from(".Rhistory")
    );
}
