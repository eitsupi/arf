//! Background fuzzy search for the help browser.

#[cfg(test)]
mod tests;

use crate::fuzzy::FuzzyScoreMatcher;
use arf_harp::help::HelpTopic;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use std::collections::HashSet;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

const MAX_RESULTS: usize = 500;
const CANCEL_CHECK_INTERVAL: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SearchResult {
    pub generation: u64,
    pub matches: Vec<(usize, u32)>,
}

struct Request {
    generation: u64,
    query: String,
}

struct TopicNames<'a> {
    index: usize,
    names: Vec<&'a str>,
}

struct SearchIndex<'a> {
    topics: &'a [HelpTopic],
    names: Vec<TopicNames<'a>>,
}

impl<'a> SearchIndex<'a> {
    fn new_cancellable(
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

struct SearchScratch {
    matcher: FuzzyScoreMatcher,
    qualified: String,
    ranked: Vec<(usize, (bool, u32))>,
}

impl SearchScratch {
    fn new() -> Self {
        Self {
            matcher: FuzzyScoreMatcher::new(),
            qualified: String::new(),
            ranked: Vec::new(),
        }
    }
}

struct State {
    generation: u64,
    request: Option<Request>,
    result: Option<SearchResult>,
    failure: Option<String>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub(super) struct SearchWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl SearchWorker {
    pub(super) fn spawn(topics: Arc<[HelpTopic]>) -> io::Result<Self> {
        Self::spawn_inner(topics, None)
    }

    #[cfg(test)]
    pub(super) fn spawn_with_hook(
        topics: Arc<[HelpTopic]>,
        hook: Arc<dyn Fn(u64) + Send + Sync>,
    ) -> io::Result<Self> {
        Self::spawn_inner(topics, Some(hook))
    }

    fn spawn_inner(
        topics: Arc<[HelpTopic]>,
        #[cfg(test)] hook: Option<Arc<dyn Fn(u64) + Send + Sync>>,
        #[cfg(not(test))] _hook: Option<()>,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                request: None,
                result: None,
                failure: None,
                shutdown: false,
            }),
            changed: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("help-search".to_string())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    worker_loop(
                        topics,
                        Arc::clone(&worker_shared),
                        #[cfg(test)]
                        hook,
                    );
                }));
                if outcome.is_err()
                    && let Ok(mut state) = worker_shared.state.lock()
                {
                    state.failure = Some("help search worker panicked".to_string());
                    state.request = None;
                    worker_shared.changed.notify_all();
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub(super) fn submit(&self, query: String) -> io::Result<u64> {
        let mut state = self.lock()?;
        if let Some(message) = &state.failure {
            return Err(io::Error::other(message.clone()));
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.result = None;
        state.failure = None;
        state.request = Some(Request { generation, query });
        self.shared.changed.notify_one();
        Ok(generation)
    }

    pub(super) fn cancel(&self) -> io::Result<u64> {
        let mut state = self.lock()?;
        state.generation = state.generation.wrapping_add(1);
        state.request = None;
        state.result = None;
        let generation = state.generation;
        self.shared.changed.notify_one();
        Ok(generation)
    }

    pub(super) fn take_result(&self, generation: u64) -> io::Result<Option<SearchResult>> {
        let mut state = self.lock()?;
        if let Some(message) = &state.failure {
            return Err(io::Error::other(message.clone()));
        }
        if state.generation != generation {
            return Ok(None);
        }
        Ok(state
            .result
            .take_if(|result| result.generation == generation))
    }

    #[cfg(test)]
    pub(super) fn wait_for_result(&self, generation: u64) -> io::Result<SearchResult> {
        let mut state = self.lock()?;
        loop {
            if let Some(message) = &state.failure {
                return Err(io::Error::other(message.clone()));
            }
            if state.generation != generation {
                return Err(io::Error::other("help search generation was superseded"));
            }
            if let Some(result) = state.result.take() {
                return Ok(result);
            }
            state = self
                .shared
                .changed
                .wait(state)
                .map_err(|_| io::Error::other("help search worker state was poisoned"))?;
        }
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, State>> {
        self.shared
            .state
            .lock()
            .map_err(|_| io::Error::other("help search worker state was poisoned"))
    }

    pub(super) fn shutdown_and_join(&mut self) -> io::Result<()> {
        self.request_shutdown();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| io::Error::other("help search worker panicked"))?;
        }
        Ok(())
    }

    pub(super) fn request_shutdown(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.shutdown = true;
            state.request = None;
            self.shared.changed.notify_one();
        }
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        let _ = self.shutdown_and_join();
    }
}

fn worker_loop(
    topics: Arc<[HelpTopic]>,
    shared: Arc<Shared>,
    #[cfg(test)] hook: Option<Arc<dyn Fn(u64) + Send + Sync>>,
) {
    let Some(index) = SearchIndex::new_cancellable(&topics, || is_shutdown(&shared)) else {
        return;
    };
    let mut scratch = SearchScratch::new();
    loop {
        let request = {
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            while state.request.is_none() && !state.shutdown {
                let Ok(next) = shared.changed.wait(state) else {
                    return;
                };
                state = next;
            }
            if state.shutdown {
                return;
            }
            state.request.take().expect("request exists after wait")
        };

        #[cfg(test)]
        if let Some(hook) = &hook {
            hook(request.generation);
        }
        let search = catch_unwind(AssertUnwindSafe(|| {
            search_topics(&index, &request.query, &mut scratch, || {
                is_cancelled(&shared, request.generation)
            })
        }));
        match search {
            Ok(Some(matches)) => {
                if let Ok(mut state) = shared.state.lock()
                    && state.generation == request.generation
                    && !state.shutdown
                {
                    state.result = Some(SearchResult {
                        generation: request.generation,
                        matches,
                    });
                    shared.changed.notify_all();
                }
            }
            Ok(None) => {}
            Err(_) => {
                if let Ok(mut state) = shared.state.lock() {
                    state.failure = Some("help search failed while matching topics".to_string());
                    state.request = None;
                }
                return;
            }
        }
    }
}

fn is_cancelled(shared: &Shared, generation: u64) -> bool {
    shared
        .state
        .lock()
        .map(|state| state.shutdown || state.generation != generation)
        .unwrap_or(true)
}

fn is_shutdown(shared: &Shared) -> bool {
    shared
        .state
        .lock()
        .map(|state| state.shutdown)
        .unwrap_or(true)
}

fn search_topics(
    index: &SearchIndex<'_>,
    query: &str,
    scratch: &mut SearchScratch,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Vec<(usize, u32)>> {
    if cancelled() {
        return None;
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
pub(super) fn search_topics_sync(topics: &[HelpTopic], query: &str) -> Vec<(usize, u32)> {
    if query.is_empty() {
        return (0..topics.len().min(MAX_RESULTS))
            .map(|index| (index, 0))
            .collect();
    }
    let index = SearchIndex::new_cancellable(topics, || false).unwrap();
    let mut scratch = SearchScratch::new();
    search_topics(&index, query, &mut scratch, || false).unwrap()
}
