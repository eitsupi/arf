//! Background fuzzy search for the help browser.

#[cfg(test)]
mod tests;

use crate::help_search::{SearchIndex, SearchScratch, search_topics};
use arf_harp::help::HelpTopic;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SearchResult {
    pub generation: u64,
    pub matches: Vec<(usize, u32)>,
}

struct Request {
    generation: u64,
    query: String,
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
