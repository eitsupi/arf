use super::super::*;
use super::support::*;

#[test]
fn parses_timestamped_modes_and_resets_mode_at_boundaries() {
    let file = text_fixture(
        r#"# time: 2024-01-15 10:30:00 UTC
# mode: r
+library(dplyr)

# time: 2024-01-15 10:31:00 UTC
# mode: shell
+ls -la

# time: 2024-01-15 10:32:00 UTC
+browse_command

# time: 2024-01-15 10:33:00 UTC
# mode: browse
+n
"#,
    );

    let parsed = parse_radian_history(file.path()).unwrap();
    assert_eq!(parsed.entries.len(), 4);
    assert_eq!(
        parsed.entries[0],
        r("library(dplyr)").at(timestamp("2024-01-15T10:30:00Z"))
    );
    assert_eq!(
        parsed.entries[1],
        shell("ls -la").at(timestamp("2024-01-15T10:31:00Z"))
    );
    assert_eq!(
        parsed.entries[2],
        entry("browse_command").at(timestamp("2024-01-15T10:32:00Z"))
    );
    assert_eq!(
        parsed.entries[3],
        browse("n").at(timestamp("2024-01-15T10:33:00Z"))
    );
}

#[test]
fn parses_multiline_commands_as_one_entry() {
    let file = text_fixture(
        r#"# time: 2024-01-15 10:30:00 UTC
# mode: r
+iris %>%
+  filter(Species == "setosa")
"#,
    );

    let parsed = parse_radian_history(file.path()).unwrap();
    assert_eq!(
        parsed.entries,
        [r(r#"iris %>%
  filter(Species == "setosa")"#)
        .at(timestamp("2024-01-15T10:30:00Z"))]
    );
}

#[test]
fn handles_empty_and_consecutive_timestamps() {
    let file = text_fixture(
        r#"# time: 2024-01-15 10:30:00 UTC
# mode: r
# time: 2024-01-15 10:31:00 UTC
# mode: shell
+pwd
"#,
    );
    let parsed = parse_radian_history(file.path()).unwrap();
    assert_eq!(
        parsed.entries,
        [shell("pwd").at(timestamp("2024-01-15T10:31:00Z"))]
    );

    let empty = text_fixture("");
    assert!(
        parse_radian_history(empty.path())
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn accepts_crlf_line_endings() {
    let line_ending = [char::from(13), char::from(10)].iter().collect::<String>();
    let contents = [
        "# time: 2024-01-15 10:30:00 UTC",
        "# mode: r",
        "+summary(iris)",
    ]
    .join(&line_ending);
    let file = text_fixture(&contents);
    assert_eq!(
        parse_radian_history(file.path()).unwrap().entries,
        [r("summary(iris)").at(timestamp("2024-01-15T10:30:00Z"))]
    );
}

#[test]
fn reports_file_not_found() {
    let path = tempfile::tempdir().unwrap().path().join("missing.radian");
    assert!(parse_radian_history(&path).is_err());
}
