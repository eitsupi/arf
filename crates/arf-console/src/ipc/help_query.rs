//! Static IPC queries for installed R help metadata.

use crate::help_search::{SearchIndex, SearchScratch, search_topics};
use crate::ipc::protocol::{
    HelpCandidate, HelpSearchParams, HelpSearchResult, HelpShowParams, HelpShowResult,
    HelpTopicResult,
};
use arf_harp::help::{
    CandidateDisplay, CurrentPackage, HelpResolution, HelpTargetResolver, HelpTopic,
    ResolveOptions, get_rd_help_topics_from_paths,
};
use arf_harp::lib_paths::cached_lib_paths;

const MAX_LIMIT: usize = 200;

#[derive(Debug)]
pub(crate) enum HelpQueryError {
    InvalidParams(String),
    EmptyLibraryPaths,
    NotFound(String),
    Internal(String),
}

/// Search installed Rd help metadata without evaluating R.
pub(crate) fn query_help_topics(
    params: &HelpSearchParams,
) -> Result<HelpSearchResult, HelpQueryError> {
    if params.limit == 0 || params.limit > MAX_LIMIT {
        return Err(HelpQueryError::InvalidParams(format!(
            "limit must be between 1 and {MAX_LIMIT}"
        )));
    }
    let library_paths = library_paths()?;
    let (topics, truncated) = search_help_topics(
        get_rd_help_topics_from_paths(&library_paths, params.package.as_deref()),
        params.query.as_deref().unwrap_or_default(),
        params.limit,
    );
    Ok(HelpSearchResult {
        topics,
        truncated,
        library_paths,
    })
}

fn search_help_topics(
    topics: Vec<HelpTopic>,
    query: &str,
    limit: usize,
) -> (Vec<HelpTopicResult>, bool) {
    let index = SearchIndex::new_cancellable(&topics, || false).expect("search is not cancelled");
    let ranked = search_topics(&index, query, &mut SearchScratch::new(), || false)
        .expect("search is not cancelled");
    let truncated = ranked.len() > limit;
    let results = ranked
        .into_iter()
        .take(limit)
        .map(|(index, _)| {
            let entry = &topics[index];
            HelpTopicResult {
                candidate: candidate_result(entry),
                aliases: entry.aliases.clone(),
            }
        })
        .collect();
    (results, truncated)
}

fn candidate_result(topic: &HelpTopic) -> HelpCandidate {
    HelpCandidate {
        package: topic.package.clone(),
        topic: topic.topic.clone(),
        help_key: topic.help_key.clone(),
        title: topic.title.clone(),
    }
}

/// Resolve and render one installed Rd help page without entering R.
pub(crate) fn query_help(params: &HelpShowParams) -> Result<HelpShowResult, HelpQueryError> {
    if params.topic.trim().is_empty() {
        return Err(HelpQueryError::InvalidParams(
            "topic must not be empty".into(),
        ));
    }
    if let Some(key) = &params.help_key
        && (key.trim().is_empty() || params.package.as_deref().is_none_or(str::is_empty))
    {
        return Err(HelpQueryError::InvalidParams(
            "help_key must not be empty and requires an explicit package".into(),
        ));
    }
    let library_paths = library_paths()?;
    let mut resolver = HelpTargetResolver::new(library_paths.clone());
    let resolution = resolver
        .resolve_target(
            &params.topic,
            params.package.as_deref(),
            params.current_package.as_deref().map(CurrentPackage::Named),
            params.help_key.as_deref(),
            ResolveOptions {
                skip_broken_indexes: true,
                candidate_display: CandidateDisplay::Metadata,
                deduplicate_keyless: false,
            },
        )
        .map_err(|error| HelpQueryError::Internal(error.to_string()))?;
    match resolution {
        HelpResolution::Page(page) => Ok(HelpShowResult::Page {
            title: resolver.title_for_page(&page),
            package: page.package,
            topic: page.display_topic,
            content: page.markdown,
            content_type: "markdown",
            entry_type: "help",
            library_paths,
        }),
        HelpResolution::Candidates(mut candidates) => {
            candidates.sort_by(|left, right| {
                (&left.package, &left.topic, &left.help_key).cmp(&(
                    &right.package,
                    &right.topic,
                    &right.help_key,
                ))
            });
            Ok(HelpShowResult::Candidates {
                candidates: candidates.iter().map(candidate_result).collect(),
                library_paths,
            })
        }
        HelpResolution::NotFound => Err(HelpQueryError::NotFound(format!(
            "Help topic '{}' was not found",
            params.topic
        ))),
    }
}

fn library_paths() -> Result<Vec<String>, HelpQueryError> {
    let paths = cached_lib_paths();
    ensure_library_paths(&paths)?;
    Ok(paths)
}

fn ensure_library_paths(paths: &[String]) -> Result<(), HelpQueryError> {
    if paths.is_empty() {
        Err(HelpQueryError::EmptyLibraryPaths)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn topic(topic: &str, aliases: &[&str], title: &str) -> arf_harp::help::HelpTopic {
        arf_harp::help::HelpTopic {
            package: "pkg".to_string(),
            package_dir: std::path::PathBuf::from("/library/pkg"),
            topic: topic.to_string(),
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            help_key: None,
            title: title.to_string(),
            entry_type: r"help".to_string(),
        }
    }

    #[test]
    fn topic_limit_is_bounded() {
        let params = HelpSearchParams {
            query: None,
            package: None,
            limit: MAX_LIMIT + 1,
        };
        assert!(matches!(
            query_help_topics(&params),
            Err(HelpQueryError::InvalidParams(_))
        ));
    }

    #[test]
    fn help_rejects_empty_topic() {
        let params = HelpShowParams {
            topic: "  ".to_string(),
            package: None,
            current_package: None,
            help_key: None,
        };
        assert!(matches!(
            query_help(&params),
            Err(HelpQueryError::InvalidParams(_))
        ));
    }

    #[test]
    fn help_key_requires_a_package_and_nonempty_key() {
        for (package, help_key) in [(None, "key"), (Some("base"), " ")] {
            let params = HelpShowParams {
                topic: "sum".to_owned(),
                package: package.map(str::to_owned),
                current_package: Some("base".to_owned()),
                help_key: Some(help_key.to_owned()),
            };
            assert!(matches!(
                query_help(&params),
                Err(HelpQueryError::InvalidParams(_))
            ));
        }
    }

    #[test]
    fn search_uses_fuzzy_ranking_before_limit_and_marks_truncated() {
        let topics = vec![
            topic(r"sprint", &[r"fast_run"], r"Sprint"),
            topic(r"print", &[r"show_value"], r"Print a value"),
        ];

        let (limited, truncated) = search_help_topics(topics.clone(), "print", 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(
            serde_json::to_value(&limited[0]).unwrap(),
            serde_json::from_str::<Value>(
                r#"{"package":"pkg","topic":"print","aliases":["show_value"],"help_key":null,"title":"Print a value"}"#
            ).unwrap()
        );
        assert!(truncated);
        assert_eq!(
            search_help_topics(topics.clone(), "show_val", 20).0[0]
                .candidate
                .topic,
            "print"
        );
        assert_eq!(
            search_help_topics(topics.clone(), "pkg::sprint", 20).0[0]
                .candidate
                .topic,
            "sprint"
        );
    }

    #[test]
    fn empty_library_snapshot_has_a_distinct_error() {
        assert!(matches!(
            ensure_library_paths(&[]),
            Err(HelpQueryError::EmptyLibraryPaths)
        ));
        assert!(ensure_library_paths(&["/library".to_string()]).is_ok());
    }
}
