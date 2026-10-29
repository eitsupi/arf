use super::{
    get_help_topics_from_paths, project_help_topic_entry, read_package_topics,
    try_get_help_topics_from_paths, vignette_topic,
};
use crate::help::fixture_path;
use rd_helpdb::VignetteEntry;

fn entry(file: &str, pdf: &str, r: &str) -> VignetteEntry {
    VignetteEntry {
        file: file.to_owned(),
        title: String::new(),
        pdf: pdf.to_owned(),
        r: r.to_owned(),
        depends: Vec::new(),
        keywords: Vec::new(),
    }
}

#[test]
fn resolves_vignette_topic_from_r_pdf_or_file() {
    assert_eq!(
        vignette_topic(&entry("ignored.Rmd", "ignored.pdf", "guide.R")),
        "guide"
    );
    assert_eq!(
        vignette_topic(&entry("ignored.Rmd", "guide.pdf", "")),
        "guide"
    );
    assert_eq!(
        vignette_topic(&entry("vignettes/guide.Rmd", "", "")),
        "guide"
    );
    assert_eq!(vignette_topic(&entry("guide", "", "")), "guide");
}

fn topic_index() -> rd_helpdb::HelpTopicIndex {
    let object = rd_helpdb::read_rds_file(fixture_path("help_topics_metadata_v3.rds"))
        .expect("metadata fixture should decode");
    rd_helpdb::HelpTopicIndex::from_object(&object).expect("metadata fixture should validate")
}

fn topic_index_with_aliases(row: usize, aliases: Vec<rd_rds::RStr>) -> rd_helpdb::HelpTopicIndex {
    let object = rd_helpdb::read_rds_file(fixture_path("help_topics_metadata_v3.rds")).unwrap();
    let (rd_rds::RValue::List(mut columns), attributes) = object.into_parts() else {
        panic!("metadata fixture root should be a list");
    };
    let names = object_names(&attributes);
    let alias_column = names
        .iter()
        .position(|name| name.as_str().unwrap().unwrap() == "Aliases")
        .unwrap();
    let rd_rds::RValue::List(mut alias_rows) = columns[alias_column].value().clone() else {
        panic!("Aliases should be a list column");
    };
    alias_rows[row] = rd_rds::RObject::from_parts(
        rd_rds::RValue::Character(aliases),
        rd_rds::Attributes::default(),
    );
    columns[alias_column] = rd_rds::RObject::from_parts(
        rd_rds::RValue::List(alias_rows),
        rd_rds::Attributes::default(),
    );
    let object = rd_rds::RObject::from_parts(rd_rds::RValue::List(columns), attributes);
    rd_helpdb::HelpTopicIndex::from_object(&object).unwrap()
}

fn install_metadata_fixture(library: &std::path::Path, package: &str) -> std::path::PathBuf {
    let package_dir = library.join(package);
    let metadata = package_dir.join("Meta");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(metadata.join("package.rds"), []).unwrap();
    std::fs::copy(
        fixture_path("help_topics_metadata_v3.rds"),
        metadata.join("Rd.rds"),
    )
    .unwrap();
    package_dir
}

#[test]
fn explicit_paths_list_topics_without_r() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    let package_dir = install_metadata_fixture(&library, "fixturepkg");

    let paths = vec![library.to_string_lossy().into_owned()];
    let topics = get_help_topics_from_paths(&paths);
    assert!(topics.iter().any(|topic| {
        topic.package == "fixturepkg" && topic.package_dir == package_dir && topic.topic == "first"
    }));
}

#[test]
fn explicit_help_paths_keep_first_installed_copy_order() {
    let temp = tempfile::tempdir().unwrap();
    let first_library = temp.path().join("first");
    let second_library = temp.path().join("second");
    let first_package = install_metadata_fixture(&first_library, "fixturepkg");
    let second_package = install_metadata_fixture(&second_library, "fixturepkg");
    let paths = vec![
        first_library.to_string_lossy().into_owned(),
        second_library.to_string_lossy().into_owned(),
    ];

    let topics = get_help_topics_from_paths(&paths);
    assert!(!topics.is_empty(), "fixture should contain help metadata");
    assert!(
        topics
            .iter()
            .all(|topic| topic.package_dir == first_package)
    );

    let reversed_topics = get_help_topics_from_paths(&paths.into_iter().rev().collect::<Vec<_>>());
    assert!(
        !reversed_topics.is_empty(),
        "fixture should contain help metadata"
    );
    assert!(
        reversed_topics
            .iter()
            .all(|topic| topic.package_dir == second_package)
    );
}

#[test]
fn strict_help_search_limits_package_reads_and_reports_corrupt_indexes() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    install_metadata_fixture(&library, "goodpkg");
    let broken_meta = library.join("brokenpkg/Meta");
    std::fs::create_dir_all(&broken_meta).unwrap();
    std::fs::write(broken_meta.join("package.rds"), []).unwrap();
    std::fs::write(broken_meta.join("Rd.rds"), b"broken RDS").unwrap();
    let paths = vec![library.to_string_lossy().into_owned()];

    let selected = try_get_help_topics_from_paths(&paths, Some("goodpkg")).unwrap();
    assert!(selected.iter().any(|topic| topic.package == "goodpkg"));
    assert!(try_get_help_topics_from_paths(&paths, None).is_err());
}

#[test]
fn help_entry_projection_prefers_matching_name_and_keeps_all_aliases() {
    let index = topic_index();
    let topic = project_help_topic_entry(
        "fixture",
        std::path::Path::new("/fixture"),
        index.entries().next().unwrap(),
    )
    .expect("first metadata row should be visible");

    assert_eq!(topic.topic, "first");
    assert_eq!(topic.aliases, ["shared", "first", "first"]);
    assert_eq!(topic.help_key.as_deref(), Some("first-topic"));
    assert_eq!(topic.title, "First topic title");
    assert_eq!(topic.package_dir, std::path::Path::new("/fixture"));
}

#[test]
fn help_entry_projection_uses_only_first_alias_when_name_is_not_an_alias() {
    let index = topic_index_with_aliases(
        2,
        vec![
            rd_rds::RStr::new(
                b"first-alias",
                rd_rds::REncoding::Utf8,
                rd_rds::NativeEncodingSource::Unknown,
            ),
            rd_rds::RStr::new(
                b"later-alias",
                rd_rds::REncoding::Utf8,
                rd_rds::NativeEncodingSource::Unknown,
            ),
        ],
    );
    let entry = index.entries().nth(2).unwrap();
    let topic =
        project_help_topic_entry("fixture", std::path::Path::new("/fixture"), entry).unwrap();
    assert_eq!(topic.topic, "first-alias");
    assert_eq!(topic.aliases, ["first-alias", "later-alias"]);
}

#[test]
fn help_entry_projection_skips_first_na_even_when_a_later_alias_is_usable() {
    let index = topic_index_with_aliases(
        0,
        vec![
            rd_rds::RStr::Na,
            rd_rds::RStr::new(
                b"later",
                rd_rds::REncoding::Utf8,
                rd_rds::NativeEncodingSource::Unknown,
            ),
        ],
    );

    assert!(
        project_help_topic_entry(
            "fixture",
            std::path::Path::new("/fixture"),
            index.entries().next().unwrap()
        )
        .is_none()
    );
}

#[test]
fn help_entry_projection_uses_empty_title_for_missing_or_na_and_skips_empty_alias_groups() {
    let index = topic_index();
    let entries: Vec<_> = index.entries().collect();
    let topic =
        project_help_topic_entry("fixture", std::path::Path::new("/fixture"), entries[1]).unwrap();
    assert_eq!(topic.title, "");
    assert_eq!(
        project_help_topic_entry("fixture", std::path::Path::new("/fixture"), entries[3]),
        None
    );

    let missing_title =
        rd_helpdb::read_rds_file(fixture_path("help_topics_aliases_only_v3.rds")).unwrap();
    let missing_index = rd_helpdb::HelpTopicIndex::from_object(&missing_title).unwrap();
    assert_eq!(
        project_help_topic_entry(
            "fixture",
            std::path::Path::new("/fixture"),
            missing_index.entries().next().unwrap()
        )
        .unwrap()
        .title,
        ""
    );
}

#[test]
fn metadata_sources_are_independent_of_compiled_help_and_each_other() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("Meta");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::copy(
        fixture_path("help_topics_metadata_v3.rds"),
        metadata.join("Rd.rds"),
    )
    .unwrap();
    std::fs::write(metadata.join("vignette.rds"), b"broken RDS").unwrap();
    std::fs::copy(fixture_path("demo_valid_v3.rds"), metadata.join("demo.rds")).unwrap();

    let topics = read_package_topics("fixture", temp.path());
    assert!(topics.iter().any(|topic| topic.entry_type == "help"));
    assert!(topics.iter().any(|topic| topic.entry_type == "demo"));
    assert!(!topics.iter().any(|topic| topic.entry_type == "vignette"));
    assert!(!temp.path().join("help").exists());

    std::fs::copy(
        fixture_path("vignette_reordered_v3.rds"),
        metadata.join("vignette.rds"),
    )
    .unwrap();
    std::fs::write(metadata.join("demo.rds"), b"broken RDS").unwrap();
    let topics = read_package_topics("fixture", temp.path());
    assert!(topics.iter().any(|topic| topic.entry_type == "help"));
    assert!(topics.iter().any(|topic| topic.entry_type == "vignette"));
    assert!(!topics.iter().any(|topic| topic.entry_type == "demo"));

    let compiled_help = temp.path().join("help");
    std::fs::create_dir(&compiled_help).unwrap();
    std::fs::write(compiled_help.join("aliases.rds"), b"broken RDS").unwrap();
    std::fs::write(compiled_help.join("fixture.rdx"), b"broken index").unwrap();
    std::fs::write(compiled_help.join("fixture.rdb"), b"broken database").unwrap();
    let topics = read_package_topics("fixture", temp.path());
    assert!(topics.iter().any(|topic| topic.entry_type == "help"));

    std::fs::write(metadata.join("Rd.rds"), b"broken RDS").unwrap();
    std::fs::copy(fixture_path("demo_valid_v3.rds"), metadata.join("demo.rds")).unwrap();
    let topics = read_package_topics("fixture", temp.path());
    assert!(!topics.iter().any(|topic| topic.entry_type == "help"));
    assert!(topics.iter().any(|topic| topic.entry_type == "vignette"));
    assert!(topics.iter().any(|topic| topic.entry_type == "demo"));
}

fn object_names(attributes: &rd_rds::Attributes) -> Vec<rd_rds::RStr> {
    attributes
        .get("names")
        .and_then(|names| match names.value() {
            rd_rds::RValue::Character(names) => Some(names.clone()),
            _ => None,
        })
        .expect("metadata names")
}
