use super::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/help_resolution")
        .join(name)
}

fn package(library: &Path, name: &str) -> PathBuf {
    let dir = library.join(name);
    std::fs::create_dir_all(dir.join("help")).unwrap();
    std::fs::create_dir_all(dir.join("Meta")).unwrap();
    for extension in ["rdx", "rdb"] {
        std::fs::copy(
            fixture(&format!("resolverpkg.{extension}")),
            dir.join(format!("help/{name}.{extension}")),
        )
        .unwrap();
    }
    std::fs::copy(fixture("aliases.rds"), dir.join("help/aliases.rds")).unwrap();
    std::fs::copy(fixture("Rd.rds"), dir.join("Meta/Rd.rds")).unwrap();
    // Library discovery only checks that this installed-package marker exists.
    std::fs::write(dir.join("Meta/package.rds"), []).unwrap();
    dir
}

fn source_package(library: &Path) -> PathBuf {
    let dir = package(library, "source");
    std::fs::copy(fixture("source_aliases.rds"), dir.join("help/aliases.rds")).unwrap();
    dir
}

fn current(dir: &Path) -> PreparedHelpPage {
    PreparedHelpPage {
        package_dir: dir.to_owned(),
        package: dir.file_name().unwrap().to_str().unwrap().to_owned(),
        display_topic: "home".to_owned(),
        help_key: "first-topic".to_owned(),
        markdown: "Original page".to_owned(),
    }
}

fn resolver(libraries: &[&Path]) -> HelpTargetResolver {
    HelpTargetResolver::new(
        libraries
            .iter()
            .map(|dir| dir.to_str().unwrap().to_owned())
            .collect(),
    )
}

fn resolve(
    resolver: &mut HelpTargetResolver,
    current: &PreparedHelpPage,
    uri: &str,
) -> HarpResult<HelpResolution> {
    resolver.resolve(current, &HelpTarget::from_uri(uri).unwrap())
}

fn page(result: HelpResolution) -> PreparedHelpPage {
    match result {
        HelpResolution::Page(page) => page,
        other => panic!("expected one prepared page, got {other:?}"),
    }
}

#[test]
fn unqualified_links_keep_the_current_copy_outside_library_order() {
    let temp = tempfile::tempdir().unwrap();
    let early = temp.path().join("early");
    let late = temp.path().join("late");
    let shadow = package(&early, "base");
    let origin = package(&late, "base");
    std::fs::write(shadow.join("help/base.rdx"), b"broken database").unwrap();
    let result = page(
        resolve(
            &mut resolver(&[&early, &late]),
            &current(&origin),
            "x-r-help:/mean",
        )
        .unwrap(),
    );
    assert_eq!(result.package_dir, origin);
    assert_eq!(result.package, "base");
    assert_eq!(result.help_key, "first-topic");
    assert_eq!(result.display_topic, "mean");
    assert!(result.markdown.contains("Fixture first-topic"));
}

#[test]
fn alias_precedence_last_wins_and_exact_key_fallback_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let dir = package(temp.path(), "base");
    let mut resolver = resolver(&[]);
    for (topic, expected) in [
        ("mean", "first-topic"),
        ("duplicate", "second-topic"),
        ("first-topic", "second-topic"),
        ("second-topic", "second-topic"),
    ] {
        let result =
            page(resolve(&mut resolver, &current(&dir), &format!("x-r-help:/{topic}")).unwrap());
        assert_eq!(result.help_key, expected);
        assert_eq!(result.display_topic, topic);
    }
}

#[test]
fn qualified_links_use_only_the_first_installed_copy() {
    let temp = tempfile::tempdir().unwrap();
    let early = temp.path().join("early");
    let late = temp.path().join("late");
    let origin = source_package(&late);
    let first = package(&early, "stats");
    package(&late, "stats");
    let mut resolver = resolver(&[&early, &late]);
    let result = page(resolve(&mut resolver, &current(&origin), "x-r-help:stats/lm").unwrap());
    assert_eq!(result.package_dir, first);
    assert_eq!(result.package, "stats");
    assert_eq!(result.help_key, "second-topic");
    std::fs::copy(
        fixture("source_aliases.rds"),
        first.join("help/aliases.rds"),
    )
    .unwrap();
    assert_eq!(
        resolve(&mut resolver, &current(&origin), "x-r-help:stats/lm").unwrap(),
        HelpResolution::NotFound
    );
}

#[test]
fn operator_aliases_resolve_without_path_or_percent_interpretation() {
    let temp = tempfile::tempdir().unwrap();
    let dir = package(temp.path(), "base");
    let mut resolver = resolver(&[temp.path()]);
    for operator in ["[", "[[", "%/%", "%in%", "/"] {
        for uri in [
            format!("x-r-help:/{operator}"),
            format!("x-r-help:base/{operator}"),
        ] {
            let result = page(resolve(&mut resolver, &current(&dir), &uri).unwrap());
            assert_eq!(result.help_key, "first-topic");
            assert_eq!(result.display_topic, operator);
        }
    }
}

#[test]
fn unique_metadata_candidate_uses_its_key_without_an_alias_file() {
    let temp = tempfile::tempdir().unwrap();
    let origin = source_package(temp.path());
    let other = package(temp.path(), "other");
    std::fs::remove_file(other.join("help/aliases.rds")).unwrap();
    let result = page(
        resolve(
            &mut resolver(&[temp.path()]),
            &current(&origin),
            "x-r-help:/mean",
        )
        .unwrap(),
    );
    assert_eq!(result.package_dir, other);
    assert_eq!(result.display_topic, "mean");
    assert_eq!(result.help_key, "first-topic");
}

#[test]
fn metadata_without_keys_resolves_aliases_within_the_selected_copy() {
    let temp = tempfile::tempdir().unwrap();
    let origin = source_package(temp.path());
    let other = package(temp.path(), "other");
    std::fs::copy(fixture("Rd_without_keys.rds"), other.join("Meta/Rd.rds")).unwrap();
    let result = page(
        resolve(
            &mut resolver(&[temp.path()]),
            &current(&origin),
            "x-r-help:/lm",
        )
        .unwrap(),
    );
    assert_eq!(result.package_dir, other);
    assert_eq!(result.help_key, "second-topic");
}

#[test]
fn ambiguous_targets_require_selection_and_prepare_only_the_chosen_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let early = temp.path().join("early");
    let late = temp.path().join("late");
    let origin = source_package(&early);
    let alpha = package(&early, "alpha");
    let beta = package(&early, "beta");
    package(&late, "alpha");
    std::fs::write(alpha.join("help/alpha.rdx"), b"broken database").unwrap();
    let result = resolve(
        &mut resolver(&[&early, &late]),
        &current(&origin),
        "x-r-help:/shared",
    )
    .unwrap();
    let HelpResolution::Candidates(candidates) = result else {
        panic!("ambiguous target must not open a guessed page");
    };
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].package_dir, alpha);
    assert_eq!(candidates[1].package_dir, beta);
    assert_eq!(candidates[1].title, "First fixture");
    let chosen = HelpTargetResolver::prepare_candidate(&candidates[1]).unwrap();
    assert_eq!(chosen.package_dir, beta);
    assert_eq!(chosen.display_topic, "shared");
    assert!(HelpTargetResolver::prepare_candidate(&candidates[0]).is_err());
}

#[test]
fn current_package_misses_do_not_jump_to_another_copy_of_the_same_package() {
    let temp = tempfile::tempdir().unwrap();
    let early = temp.path().join("early");
    let late = temp.path().join("late");
    package(&early, "source");
    let origin = source_package(&late);
    assert_eq!(
        resolve(
            &mut resolver(&[&early, &late]),
            &current(&origin),
            "x-r-help:/mean"
        )
        .unwrap(),
        HelpResolution::NotFound
    );
}

#[test]
fn missing_targets_and_packages_are_distinct_from_read_errors() {
    let temp = tempfile::tempdir().unwrap();
    let origin = source_package(temp.path());
    let mut resolver = resolver(&[temp.path()]);
    for uri in [
        "x-r-help:/absent",
        "x-r-help:missing/mean",
        "x-r-help:source/absent",
    ] {
        assert_eq!(
            resolve(&mut resolver, &current(&origin), uri).unwrap(),
            HelpResolution::NotFound
        );
    }
    std::fs::write(origin.join("help/source.rdb"), b"corrupt record").unwrap();
    assert!(
        matches!(resolve(&mut resolver, &current(&origin), "x-r-help:/home"), Err(HarpError::HelpDatabase { package, key, .. }) if package == "source" && key == "first-topic")
    );
}

#[test]
fn corrupt_aliases_abort_instead_of_searching_other_packages() {
    let temp = tempfile::tempdir().unwrap();
    let origin = source_package(temp.path());
    package(temp.path(), "other");
    std::fs::write(origin.join("help/aliases.rds"), b"corrupt aliases").unwrap();
    assert!(
        resolve(
            &mut resolver(&[temp.path()]),
            &current(&origin),
            "x-r-help:/mean"
        )
        .is_err()
    );
}

#[test]
fn metadata_is_lazy_and_corruption_is_reported_without_guessing() {
    let temp = tempfile::tempdir().unwrap();
    let origin = source_package(temp.path());
    let other = package(temp.path(), "other");
    std::fs::write(other.join("Meta/Rd.rds"), b"corrupt metadata").unwrap();
    let mut resolver = resolver(&[temp.path()]);
    assert_eq!(
        page(resolve(&mut resolver, &current(&origin), "x-r-help:/home").unwrap()).package_dir,
        origin
    );
    assert!(
        matches!(resolve(&mut resolver, &current(&origin), "x-r-help:/mean"), Err(HarpError::HelpDatabase { package, .. }) if package == "other")
    );
}

#[test]
fn packages_without_topic_metadata_do_not_create_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let origin = source_package(temp.path());
    let other = package(temp.path(), "other");
    std::fs::remove_file(other.join("Meta/Rd.rds")).unwrap();
    assert_eq!(
        resolve(
            &mut resolver(&[temp.path()]),
            &current(&origin),
            "x-r-help:/mean"
        )
        .unwrap(),
        HelpResolution::NotFound
    );
}
