use super::*;
use crate::fuzzy::fuzzy_match_with_case_preference;
use arf_harp::help::HelpTopic;

fn topic(package: &str, name: &str, aliases: &[&str], title: &str) -> HelpTopic {
    HelpTopic {
        package: package.to_owned(),
        package_dir: std::path::PathBuf::from(format!("/{package}")),
        topic: name.to_owned(),
        aliases: aliases.iter().map(|value| (*value).to_owned()).collect(),
        help_key: None,
        title: title.to_owned(),
        entry_type: "help".to_owned(),
    }
}

fn fuzzy_search_topics(topics: &[HelpTopic], query: &str) -> Vec<(HelpTopic, u32)> {
    search_topics_sync(topics, query)
        .into_iter()
        .map(|(index, score)| (topics[index].clone(), score))
        .collect()
}

#[test]
fn test_fuzzy_search_topics() {
    let topics = [
        HelpTopic {
            package: "base".into(),
            package_dir: "/base".into(),
            topic: "print".into(),
            aliases: vec!["print.default".into()],
            help_key: Some("print".into()),
            title: "Print Values".into(),
            entry_type: "help".into(),
        },
        HelpTopic {
            package: "dplyr".into(),
            package_dir: "/dplyr".into(),
            topic: "mutate".into(),
            aliases: vec![],
            help_key: None,
            title: "Create, modify, and delete columns".into(),
            entry_type: "help".into(),
        },
    ];
    assert_eq!(fuzzy_search_topics(&topics, "print")[0].0.topic, "print");
    assert_eq!(fuzzy_search_topics(&topics, "mut")[0].0.topic, "mutate");
}

#[test]
fn fuzzy_search_prefers_matching_case_without_filtering_variants() {
    let topics = [
        topic("base", "foo_bar", &[], "Lowercase topic"),
        topic("base", "Foo", &[], "Capitalized topic"),
    ];
    let lowercase = fuzzy_search_topics(&topics, "foo");
    assert!(lowercase.iter().any(|(item, _)| item.topic == "foo_bar"));
    assert!(lowercase.iter().any(|(item, _)| item.topic == "Foo"));
    let uppercase = fuzzy_search_topics(&topics, "Foo");
    assert_eq!(uppercase.len(), 2);
    assert_eq!(uppercase[0].0.topic, "Foo");
    assert_eq!(uppercase[1].0.topic, "foo_bar");
}

#[test]
fn test_fuzzy_search_topics_empty_query() {
    let topics = [topic("base", "print", &[], "Print Values")];
    assert_eq!(fuzzy_search_topics(&topics, "").len(), 1);
}

#[test]
fn test_fuzzy_search_topics_no_match() {
    let topics = [topic("base", "print", &[], "Print Values")];
    assert!(fuzzy_search_topics(&topics, "xyz123").is_empty());
}

#[test]
fn fuzzy_search_matches_aliases_without_duplicating_topic_results() {
    let item = HelpTopic {
        package: "base".into(),
        package_dir: "/base".into(),
        topic: "print".into(),
        aliases: vec!["print.default".into(), "print.value".into()],
        help_key: Some("print".into()),
        title: "Print Values".into(),
        entry_type: "help".into(),
    };
    let results = fuzzy_search_topics(std::slice::from_ref(&item), "print.value");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.topic, "print");
    assert_eq!(
        fuzzy_search_topics(&[item], "base::print.value")[0].0.topic,
        "print"
    );
}

#[test]
fn fuzzy_search_matches_help_key_bare_and_qualified_without_changing_display_topic() {
    let item = HelpTopic {
        package: "base".into(),
        package_dir: "/base".into(),
        topic: "[.data.frame".into(),
        aliases: vec!["[.data.frame".into()],
        help_key: Some("Extract.data.frame".into()),
        title: "Extract or Replace Parts of an Object".into(),
        entry_type: "help".into(),
    };
    for query in ["Extract.data.frame", "base::Extract.data.frame"] {
        let results = fuzzy_search_topics(std::slice::from_ref(&item), query);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.topic, "[.data.frame");
    }
}

#[test]
fn fuzzy_search_keeps_source_order_for_equal_ranks() {
    let topics = [
        topic("first", "needle", &[], ""),
        topic("second", "needle", &[], ""),
    ];
    let results = fuzzy_search_topics(&topics, "needle");
    assert_eq!(
        results
            .iter()
            .map(|(item, _)| item.package.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn fuzzy_search_weights_titles_by_half() {
    let item = topic("pkg", "other", &[], "Needle");
    let results = fuzzy_search_topics(std::slice::from_ref(&item), "Needle");
    let raw = crate::fuzzy::fuzzy_match_smart_case("Needle", "Needle")
        .unwrap()
        .score;
    assert_eq!(results[0].1, raw / 2);
}

#[test]
fn fuzzy_search_prefers_smart_title_over_ignore_case_bare_name() {
    let topics = [
        topic("pkg", "unrelated", &[], "Foo"),
        topic("pkg", "foo", &[], ""),
    ];
    let results = fuzzy_search_topics(&topics, "Foo");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0.topic, "unrelated");
    assert_eq!(results[1].0.topic, "foo");
}

#[test]
fn fuzzy_search_keeps_dollar_literal_and_matches_unicode() {
    let topics = [
        topic("base", "Extract$", &[], ""),
        topic("base", "Extract", &[], ""),
        topic("unicode", "café_日本語", &[], ""),
    ];
    assert_eq!(
        fuzzy_search_topics(&topics, "Extract$")[0].0.topic,
        "Extract$"
    );
    assert_eq!(
        fuzzy_search_topics(&topics, "cafe\u{301}日本語")[0].0.topic,
        "café_日本語"
    );
}

#[test]
fn fuzzy_search_does_not_join_separate_aliases() {
    let item = topic("pkg", "other", &["foo", "bar"], "");
    for query in ["fb", "f b"] {
        assert!(
            fuzzy_search_topics(std::slice::from_ref(&item), query).is_empty(),
            "query {query:?}"
        );
    }
}

#[test]
fn fuzzy_search_limits_before_cloning_and_empty_search_keeps_first_topics() {
    let topics: Vec<_> = (0..MAX_RESULTS + 25)
        .map(|i| topic("pkg", &format!("same{i}"), &[], ""))
        .collect();
    let empty = fuzzy_search_topics(&topics, "");
    assert_eq!(empty.len(), MAX_RESULTS);
    assert_eq!(empty[0].0.topic, "same0");
    assert_eq!(empty[MAX_RESULTS - 1].0.topic, "same499");
    let matches = fuzzy_search_topics(&topics, "same");
    assert_eq!(matches.len(), MAX_RESULTS);
    assert_eq!(matches[0].0.topic, "same0");
}

#[test]
fn score_only_search_preserves_legacy_ranking_for_representative_queries() {
    let topics = [
        topic("base", "print", &["print.default"], "Print values"),
        topic("stats", "median", &["middle"], "Sample quantiles"),
        topic("pkg", "foo", &[], "Foo reference"),
        topic("pkg", "other", &[], "Foo overview"),
    ];
    for query in ["print", "middle", "Foo", "base::print.default", "xyz"] {
        assert_eq!(
            search_topics_sync(&topics, query),
            reference_search(&topics, query),
            "query {query:?}"
        );
    }
}

fn reference_search(topics: &[HelpTopic], query: &str) -> Vec<(usize, u32)> {
    let mut ranked = Vec::new();
    for (index, item) in topics.iter().enumerate() {
        let names = std::iter::once(item.topic.as_str())
            .chain(item.aliases.iter().map(String::as_str))
            .chain(item.help_key.iter().map(String::as_str));
        let mut candidates: Vec<_> = names.clone().map(|name| (name.to_owned(), 1)).collect();
        candidates.extend(names.map(|name| (format!("{}::{name}", item.package), 1)));
        candidates.push((item.title.clone(), 2));
        let best = candidates
            .iter()
            .filter_map(|(name, weight)| {
                fuzzy_match_with_case_preference(query, name)
                    .map(|matched| (matched.case_preferred, matched.fuzzy_match.score / weight))
            })
            .max();
        if let Some(rank) = best {
            ranked.push((index, rank));
        }
    }
    ranked.sort_by_key(|(_, rank)| std::cmp::Reverse(*rank));
    ranked
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(index, (_, score))| (index, score))
        .collect()
}

#[test]
fn cancellation_is_checked_during_large_alias_lists() {
    let topics = vec![HelpTopic {
        package: "pkg".to_owned(),
        package_dir: Default::default(),
        topic: "unrelated".to_owned(),
        aliases: (0..100).map(|index| format!("alias_{index}")).collect(),
        help_key: None,
        title: String::new(),
        entry_type: "help".to_owned(),
    }];
    let index = SearchIndex::new_cancellable(&topics, || false).unwrap();
    let mut scratch = SearchScratch::new();
    let mut checks = 0;
    let result = search_topics(&index, "no-match", &mut scratch, || {
        checks += 1;
        checks == 3
    });
    assert!(result.is_none());
    assert_eq!(checks, 3);
}

#[test]
fn shutdown_is_checked_during_initial_alias_preparation() {
    let topics = vec![HelpTopic {
        package: "pkg".to_owned(),
        package_dir: Default::default(),
        topic: "topic".to_owned(),
        aliases: (0..100).map(|index| format!("alias_{index}")).collect(),
        help_key: None,
        title: String::new(),
        entry_type: "help".to_owned(),
    }];
    let mut checks = 0;
    let index = SearchIndex::new_cancellable(&topics, || {
        checks += 1;
        checks == 2
    });
    assert!(index.is_none());
    assert_eq!(checks, 2);
}
