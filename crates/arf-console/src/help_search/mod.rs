//! Pure fuzzy search core shared by help browser search workers.

#[cfg(test)]
mod tests;

use crate::fuzzy::FuzzyScoreMatcher;
use arf_harp::help::HelpTopic;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use std::collections::HashSet;

const MAX_RESULTS: usize = 500;
const CANCEL_CHECK_INTERVAL: usize = 32;

struct TopicNames<'a> {
    index: usize,
    names: Vec<&'a str>,
}

pub(crate) struct SearchIndex<'a> {
    topics: &'a [HelpTopic],
    names: Vec<TopicNames<'a>>,
}

impl<'a> SearchIndex<'a> {
    pub(crate) fn new_cancellable(
        topics: &'a [HelpTopic],
        mut cancelled: impl FnMut() -> bool,
    ) -> Option<Self> {
        if cancelled() {
            return None;
        }
        let mut names_by_topic = Vec::with_capacity(topics.len());
        let mut checked_names = 0;
        for (index, topic) in topics.iter().enumerate() {
            let mut seen = HashSet::new();
            let mut names = Vec::new();
            for name in std::iter::once(topic.topic.as_str())
                .chain(topic.aliases.iter().map(String::as_str))
                .chain(topic.help_key.as_deref())
            {
                checked_names += 1;
                if checked_names % CANCEL_CHECK_INTERVAL == 0 && cancelled() {
                    return None;
                }
                if seen.insert(name) {
                    names.push(name);
                }
            }
            names_by_topic.push(TopicNames { index, names });
        }
        if cancelled() {
            return None;
        }
        Some(Self {
            topics,
            names: names_by_topic,
        })
    }
}

pub(crate) struct SearchScratch {
    matcher: FuzzyScoreMatcher,
    qualified: String,
    ranked: Vec<(usize, (bool, u32))>,
}

impl SearchScratch {
    pub(crate) fn new() -> Self {
        Self {
            matcher: FuzzyScoreMatcher::new(),
            qualified: String::new(),
            ranked: Vec::new(),
        }
    }
}

pub(crate) fn search_topics(
    index: &SearchIndex<'_>,
    query: &str,
    scratch: &mut SearchScratch,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Vec<(usize, u32)>> {
    if cancelled() {
        return None;
    }
    if query.is_empty() {
        return Some(
            (0..index.topics.len().min(MAX_RESULTS))
                .map(|index| (index, 0))
                .collect(),
        );
    }
    let smart_pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let ignore_pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let same_atoms = smart_pattern.atoms == ignore_pattern.atoms;
    if cancelled() {
        return None;
    }
    scratch.ranked.clear();
    let mut checked = 0;
    for topic_names in &index.names {
        let topic = &index.topics[topic_names.index];
        let mut best_rank: Option<(bool, u32)> = None;
        let mut consider = |candidate: &str, weight: u32| {
            checked += 1;
            if checked % CANCEL_CHECK_INTERVAL == 0 && cancelled() {
                return false;
            }
            let smart_score = scratch.matcher.score(&smart_pattern, candidate);
            let (case_preferred, score) = match smart_score {
                Some(score) => (true, score),
                None if !same_atoms => match scratch.matcher.score(&ignore_pattern, candidate) {
                    Some(score) => (false, score),
                    None => return true,
                },
                None => return true,
            };
            let rank = (case_preferred, score / weight);
            best_rank = Some(best_rank.map_or(rank, |best| best.max(rank)));
            true
        };

        for name in &topic_names.names {
            if !consider(name, 1) {
                return None;
            }
            scratch.qualified.clear();
            scratch.qualified.push_str(&topic.package);
            scratch.qualified.push_str("::");
            scratch.qualified.push_str(name);
            if !consider(&scratch.qualified, 1) {
                return None;
            }
        }
        if !consider(&topic.title, 2) {
            return None;
        }
        if let Some(rank) = best_rank {
            scratch.ranked.push((topic_names.index, rank));
        }
        if checked % CANCEL_CHECK_INTERVAL == 0 && cancelled() {
            return None;
        }
    }
    if cancelled() {
        return None;
    }
    scratch
        .ranked
        .sort_by_key(|entry| std::cmp::Reverse(entry.1));
    if cancelled() {
        return None;
    }
    Some(
        scratch
            .ranked
            .iter()
            .take(MAX_RESULTS)
            .map(|(index, (_, score))| (*index, *score))
            .collect(),
    )
}

#[cfg(test)]
pub(crate) fn search_topics_sync(topics: &[HelpTopic], query: &str) -> Vec<(usize, u32)> {
    let index = SearchIndex::new_cancellable(topics, || false).unwrap();
    let mut scratch = SearchScratch::new();
    search_topics(&index, query, &mut scratch, || false).unwrap()
}
