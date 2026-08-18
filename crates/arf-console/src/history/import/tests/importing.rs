use super::super::*;
use super::support::*;

#[test]
fn planner_classifies_routing_and_input_edges() {
    let cases = [
        (r("r"), EntryPlan::Insert(ImportTarget::R)),
        (shell("shell"), EntryPlan::Insert(ImportTarget::Shell)),
        (browse("browse"), EntryPlan::Insert(ImportTarget::R)),
        (entry("unspecified"), EntryPlan::Insert(ImportTarget::R)),
        (
            entry("unsupported").with_mode(ImportMode::Unsupported("python".to_owned())),
            EntryPlan::SkipUnsupported {
                mode: "python".to_owned(),
            },
        ),
        (entry(r" 	"), EntryPlan::SkipEmpty),
    ];

    for (input, expected) in cases {
        assert_eq!(plan_entry(&input, None, None), expected);
    }
}

#[test]
fn imports_r_and_shell_items_with_all_fields() {
    let r_item = r("summary(iris)")
        .at(timestamp("2024-06-15T14:30:45Z"))
        .with_metadata(r#"{"future":true}"#)
        .with_standard_fields();
    let shell_item = shell("git status")
        .at(timestamp("2024-06-15T14:31:45Z"))
        .with_standard_fields();
    let expected_r = idless(r_item.item.clone());
    let expected_shell = idless(shell_item.item.clone());

    let mut fixture = ImportFixture::new();
    let result = fixture.import([r_item, shell_item]);

    assert_eq!(
        result,
        ImportResult {
            r_imported: 1,
            shell_imported: 1,
            ..ImportResult::default()
        }
    );
    assert_eq!(idless(fixture.r_items().remove(0)), expected_r);
    assert_eq!(idless(fixture.shell_items().remove(0)), expected_shell);
}

#[test]
fn hostname_override_replaces_source_hostname() {
    let mut fixture = ImportFixture::new();
    fixture.import_with(
        [r("hostname").with_hostname("source-host")],
        ImportOptions {
            hostname_override: Some("import-host"),
            skip_duplicates: false,
        },
    );
    assert_eq!(
        fixture.r_items()[0].hostname.as_deref(),
        Some("import-host")
    );
}

#[test]
fn radian_entries_reach_the_separate_target_databases() {
    let file = text_fixture(
        r#"# time: 2024-01-15 10:30:00 UTC
# mode: r
+summary(iris)

# time: 2024-01-15 10:31:00 UTC
# mode: shell
+pwd
"#,
    );
    let parsed = parse_radian_history(file.path()).unwrap();
    let mut fixture = ImportFixture::new();
    let result = fixture.import(parsed.entries);

    assert_eq!(result.r_imported, 1);
    assert_eq!(result.shell_imported, 1);
    assert_eq!(fixture.r_items()[0].command_line, "summary(iris)");
    assert_eq!(fixture.shell_items()[0].command_line, "pwd");
}

#[test]
fn dry_run_and_real_import_share_counts_and_warnings() {
    let entries = vec![
        r("r"),
        shell("shell"),
        entry(" "),
        entry("unknown").with_mode(ImportMode::Unsupported("python".to_owned())),
    ];
    let dry = import_entries_dry_run(&entries, None, None);
    let mut fixture = ImportFixture::new();
    let real = fixture.import(entries);
    assert_eq!(dry, real);
}
