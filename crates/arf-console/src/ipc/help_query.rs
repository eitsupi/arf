//! Static IPC queries for installed R help metadata.

use crate::ipc::protocol::{HelpSearchParams, HelpShowParams};
use arf_harp::help::{HelpTargetResolver, IpcHelpResolution, try_get_help_topics_from_paths};
use arf_harp::lib_paths::cached_lib_paths;
use serde::Serialize;

const MAX_LIMIT: usize = 200;

#[derive(Debug)]
pub(crate) enum HelpQueryError {
    InvalidParams(String),
    EmptyLibraryPaths,
    NotFound(String),
    Internal(String),
}

#[derive(Debug, Serialize)]
pub(crate) struct HelpTopicResult {
    pub package: String,
    pub topic: String,
    pub aliases: Vec<String>,
    pub title: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct HelpTopicsResult {
    pub topics: Vec<HelpTopicResult>,
    pub library_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum HelpResult {
    Page {
        package: String,
        topic: String,
        title: String,
        content: String,
        content_type: &'static str,
        entry_type: &'static str,
        library_paths: Vec<String>,
    },
    Candidates {
        candidates: Vec<HelpCandidateResult>,
        library_paths: Vec<String>,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct HelpCandidateResult {
    pub package: String,
    pub topic: String,
    pub help_key: Option<String>,
    pub title: String,
}

/// Search installed Rd help metadata without evaluating R.
pub(crate) fn query_help_topics(
    params: &HelpSearchParams,
) -> Result<HelpTopicsResult, HelpQueryError> {
    if params.limit == 0 || params.limit > MAX_LIMIT {
        return Err(HelpQueryError::InvalidParams(format!(
            "limit must be between 1 and {MAX_LIMIT}"
        )));
    }
    let library_paths = cached_lib_paths();
    ensure_library_paths(&library_paths)?;
    let matching_topics = try_get_help_topics_from_paths(&library_paths, params.package.as_deref())
        .map_err(|error| HelpQueryError::Internal(error.to_string()))?;
    let topics = search_topics(
        matching_topics,
        params.query.as_deref().unwrap_or_default(),
        params.limit,
    );

    Ok(HelpTopicsResult {
        topics,
        library_paths,
    })
}

fn search_topics(
    topics: Vec<arf_harp::help::HelpTopic>,
    query: &str,
    limit: usize,
) -> Vec<HelpTopicResult> {
    let query = query.to_lowercase();
    topics
        .into_iter()
        .filter(|entry| entry.entry_type == "help")
        .filter(|entry| {
            query.is_empty()
                || entry.topic.to_lowercase().contains(&query)
                || entry.title.to_lowercase().contains(&query)
                || entry
                    .aliases
                    .iter()
                    .any(|alias| alias.to_lowercase().contains(&query))
        })
        .take(limit)
        .map(|entry| HelpTopicResult {
            package: entry.package,
            topic: entry.topic,
            aliases: entry.aliases,
            title: entry.title,
        })
        .collect()
}

/// Resolve and render one installed Rd help page without entering R.
pub(crate) fn query_help(params: &HelpShowParams) -> Result<HelpResult, HelpQueryError> {
    if params.topic.trim().is_empty() {
        return Err(HelpQueryError::InvalidParams(
            "topic must not be empty".to_string(),
        ));
    }
    let library_paths = cached_lib_paths();
    ensure_library_paths(&library_paths)?;
    let mut resolver = HelpTargetResolver::new(library_paths.clone());
    let resolution = resolver
        .resolve_ipc_target(
            &params.topic,
            params.package.as_deref(),
            params.current_package.as_deref(),
        )
        .map_err(|error| HelpQueryError::Internal(error.to_string()))?;
    match resolution {
        IpcHelpResolution::Page { page, title } => Ok(HelpResult::Page {
            package: page.package,
            topic: page.display_topic,
            title,
            content: page.markdown,
            content_type: "markdown",
            entry_type: "help",
            library_paths,
        }),
        IpcHelpResolution::Candidates(candidates) => Ok(HelpResult::Candidates {
            candidates: candidates
                .into_iter()
                .map(|candidate| HelpCandidateResult {
                    package: candidate.package,
                    topic: candidate.topic,
                    help_key: candidate.help_key,
                    title: candidate.title,
                })
                .collect(),
            library_paths,
        }),
        IpcHelpResolution::NotFound => Err(HelpQueryError::NotFound(format!(
            "Help topic '{}' was not found",
            params.topic
        ))),
    }
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

    fn topic(
        topic: &str,
        aliases: &[&str],
        title: &str,
        entry_type: &str,
    ) -> arf_harp::help::HelpTopic {
        arf_harp::help::HelpTopic {
            package: "pkg".to_string(),
            package_dir: std::path::PathBuf::from("/library/pkg"),
            topic: topic.to_string(),
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            help_key: None,
            title: title.to_string(),
            entry_type: entry_type.to_string(),
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
        };
        assert!(matches!(
            query_help(&params),
            Err(HelpQueryError::InvalidParams(_))
        ));
    }

    #[test]
    fn search_matches_topic_alias_and_title_case_insensitively_and_limits_results() {
        let topics = vec![
            topic("first", &["AliasOne"], "A title", "help"),
            topic("second", &["other"], "SECOND title", "help"),
            topic("guide", &["AliasGuide"], "Guide title", "vignette"),
        ];

        for query in ["FIRST", "aliasone", "A TITLE", "second title"] {
            assert_eq!(search_topics(topics.clone(), query, 20).len(), 1, "{query}");
        }
        assert!(search_topics(topics.clone(), "aliasguide", 20).is_empty());
        let all = search_topics(topics, "", 1);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].topic, "first");
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
