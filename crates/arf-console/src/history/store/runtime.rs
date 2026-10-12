//! Runtime history lifecycle: persistent, volatile fallback, or unavailable.

use super::HistoryStore;
use super::receipt::{HistorySaveOutcome, HistorySaveReceipt};
use crate::history::artifact::HistoryKind;
use reedline::{HistorySessionId, Reedline, Result};
use std::path::PathBuf;

/// The store and save receipt associated with one reedline editor.
#[derive(Clone)]
pub struct HistoryHandle {
    pub store: HistoryStore,
    pub receipt: HistorySaveReceipt,
}

/// Runtime history lifecycle. Keeping this state explicit prevents a failed
/// persistent open from being confused with an intentional volatile session.
#[derive(Clone)]
pub enum HistoryRuntime {
    Persistent(HistoryHandle),
    Volatile {
        handle: HistoryHandle,
        reason: VolatileHistoryReason,
    },
    Unavailable {
        failure: HistoryFailureDetail,
        previous_failure: Option<HistoryFailureDetail>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VolatileHistoryReason {
    Configured,
    Fallback {
        persistent_failure: HistoryFailureDetail,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryFailureDetail {
    stage: HistoryFailureStage,
    path: Option<PathBuf>,
    message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HistoryFailureStage {
    PersistentPathResolution,
    PersistentOpen,
    MemoryInitialization,
}

impl HistoryRuntime {
    /// Construct the complete history lifecycle decision for one runtime.
    ///
    /// Persistent open failures (including an unavailable default path) are
    /// deliberately represented as volatile fallbacks.  Only failure to
    /// create that fallback reaches `Unavailable`.
    pub(crate) fn initialize(
        mode: &crate::config::HistoryMode,
        requested_path: Option<PathBuf>,
        history_kind: HistoryKind,
        session_id: Option<HistorySessionId>,
        session_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        Self::initialize_with_factories(
            mode,
            requested_path,
            history_kind,
            session_id,
            session_timestamp,
            HistoryStore::open,
            HistoryStore::in_memory,
        )
    }

    /// Apply the lifecycle decision with injectable backend factories.
    ///
    /// The factories are kept private so production callers cannot bypass the
    /// single runtime construction path. Unit tests use this helper to cover
    /// initialization failures that the operating system cannot reliably
    /// reproduce on every platform.
    fn initialize_with_factories<Open, Memory>(
        mode: &crate::config::HistoryMode,
        requested_path: Option<PathBuf>,
        history_kind: HistoryKind,
        session_id: Option<HistorySessionId>,
        session_timestamp: Option<chrono::DateTime<chrono::Utc>>,
        mut open: Open,
        mut in_memory: Memory,
    ) -> Self
    where
        Open: FnMut(
            PathBuf,
            HistoryKind,
            Option<HistorySessionId>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<HistoryStore>,
        Memory: FnMut(
            Option<HistorySessionId>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<HistoryStore>,
    {
        let make_handle = |store: HistoryStore| HistoryHandle {
            store,
            receipt: HistorySaveReceipt::new(),
        };

        if matches!(mode, crate::config::HistoryMode::Volatile) {
            return match in_memory(session_id, session_timestamp) {
                Ok(store) => Self::Volatile {
                    handle: make_handle(store),
                    reason: VolatileHistoryReason::Configured,
                },
                Err(error) => Self::Unavailable {
                    failure: HistoryFailureDetail::memory(error),
                    previous_failure: None,
                },
            };
        }

        let persistent_failure = match requested_path.clone() {
            Some(path) => match open(path.clone(), history_kind, session_id, session_timestamp) {
                Ok(store) => return Self::Persistent(make_handle(store)),
                Err(error) => HistoryFailureDetail::persistent_open(path, error),
            },
            None => HistoryFailureDetail::persistent_path(),
        };

        match in_memory(session_id, session_timestamp) {
            Ok(store) => Self::Volatile {
                handle: make_handle(store),
                reason: VolatileHistoryReason::Fallback { persistent_failure },
            },
            Err(error) => Self::Unavailable {
                failure: HistoryFailureDetail::memory(error),
                previous_failure: Some(persistent_failure),
            },
        }
    }

    /// Install this runtime's owned adapter on an editor, if available.
    pub fn attach_to_editor(&self, line_editor: Reedline) -> Reedline {
        let Some(handle) = self.handle() else {
            return line_editor;
        };
        let adapter = crate::history::ReedlineHistoryAdapter::new(
            handle.store.clone(),
            handle.receipt.clone(),
        );
        line_editor
            .with_history_session_id(handle.store.session())
            .with_history(Box::new(adapter))
    }

    pub fn handle(&self) -> Option<&HistoryHandle> {
        match self {
            Self::Persistent(handle) | Self::Volatile { handle, .. } => Some(handle),
            Self::Unavailable { .. } => None,
        }
    }

    pub fn store(&self) -> Option<HistoryStore> {
        self.handle().map(|handle| handle.store.clone())
    }

    pub fn receipt_outcome(&self) -> Option<HistorySaveOutcome> {
        self.handle().and_then(|handle| handle.receipt.take())
    }

    pub fn is_available(&self) -> bool {
        self.handle().is_some()
    }

    /// Stable state label for diagnostics and machine-readable startup info.
    pub fn state_name(&self) -> &'static str {
        match self {
            Self::Persistent(_) => "persistent",
            Self::Volatile { .. } => "volatile",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Stable machine-readable reason for a non-persistent runtime.
    pub fn reason_name(&self) -> Option<&'static str> {
        match self {
            Self::Persistent(_) => None,
            Self::Volatile { reason, .. } => Some(match reason {
                VolatileHistoryReason::Configured => "configured",
                VolatileHistoryReason::Fallback { .. } => "fallback",
            }),
            Self::Unavailable { .. } => Some("initialization_failed"),
        }
    }

    /// Human-readable startup warning for degraded runtimes.
    ///
    /// Persistent and intentionally configured volatile runtimes are healthy
    /// states and therefore do not produce a warning.
    pub fn startup_warning(&self) -> Option<String> {
        let detail = self.diagnostic_detail()?;
        Some(match self.requested_path() {
            Some(path) => format!("{detail} (requested path: {})", path.display()),
            None => detail,
        })
    }

    /// Human-readable detail shared by startup warnings, `:info`, and JSON.
    pub fn diagnostic_detail(&self) -> Option<String> {
        match self {
            Self::Persistent(_) => None,
            Self::Volatile { reason, .. } => match reason {
                VolatileHistoryReason::Configured => None,
                VolatileHistoryReason::Fallback { persistent_failure } => Some(format!(
                    "{}; using volatile fallback",
                    persistent_failure.describe(),
                )),
            },
            Self::Unavailable {
                failure,
                previous_failure,
            } => {
                let mut details = previous_failure
                    .as_ref()
                    .map(|failure| failure.describe())
                    .unwrap_or_default();
                if !details.is_empty() {
                    details.push_str("; ");
                }
                details.push_str(&failure.describe());
                Some(details)
            }
        }
    }

    pub fn requested_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Persistent(handle) => handle.store.path(),
            Self::Volatile { reason, .. } => match reason {
                VolatileHistoryReason::Configured => None,
                VolatileHistoryReason::Fallback { persistent_failure } => persistent_failure.path(),
            },
            Self::Unavailable {
                previous_failure, ..
            } => previous_failure
                .as_ref()
                .and_then(HistoryFailureDetail::path),
        }
    }
}

impl HistoryFailureDetail {
    #[cfg(test)]
    pub(crate) fn test_memory() -> Self {
        Self {
            stage: HistoryFailureStage::MemoryInitialization,
            path: None,
            message: "test memory initialization failure".to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_persistent_open(path: PathBuf) -> Self {
        Self {
            stage: HistoryFailureStage::PersistentOpen,
            path: Some(path),
            message: "test persistent open failure".to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_path_resolution() -> Self {
        Self::persistent_path()
    }

    fn persistent_path() -> Self {
        Self {
            stage: HistoryFailureStage::PersistentPathResolution,
            path: None,
            message: "no persistent history path is available".to_string(),
        }
    }

    fn persistent_open(path: PathBuf, error: reedline::ReedlineError) -> Self {
        Self {
            stage: HistoryFailureStage::PersistentOpen,
            path: Some(path),
            message: error.to_string(),
        }
    }

    fn memory(error: reedline::ReedlineError) -> Self {
        Self {
            stage: HistoryFailureStage::MemoryInitialization,
            path: None,
            message: error.to_string(),
        }
    }

    fn describe(&self) -> String {
        match self.stage {
            HistoryFailureStage::PersistentPathResolution => {
                format!(
                    "persistent history path resolution failed: {}",
                    self.message
                )
            }
            HistoryFailureStage::PersistentOpen => {
                format!("persistent history open failed: {}", self.message)
            }
            HistoryFailureStage::MemoryInitialization => {
                format!("volatile history initialization failed: {}", self.message)
            }
        }
    }

    fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injected_failure(message: &str) -> Result<HistoryStore> {
        Err(reedline::ReedlineError(
            reedline::ReedlineErrorVariants::HistoryDatabaseError(message.to_string()),
        ))
    }

    #[test]
    fn configured_volatile_memory_failure_is_unavailable_without_previous_failure() {
        let runtime = HistoryRuntime::initialize_with_factories(
            &crate::config::HistoryMode::Volatile,
            None,
            HistoryKind::R,
            None,
            None,
            |_path, _kind, _session, _timestamp| injected_failure("persistent must not be opened"),
            |_session, _timestamp| injected_failure("configured memory failure"),
        );

        assert!(runtime.requested_path().is_none());
        let HistoryRuntime::Unavailable {
            failure,
            previous_failure,
        } = runtime
        else {
            panic!("configured volatile initialization should be unavailable");
        };
        assert_eq!(failure.stage, HistoryFailureStage::MemoryInitialization);
        assert!(failure.message.contains("configured memory failure"));
        assert!(previous_failure.is_none());
    }

    #[test]
    fn persistent_failure_and_memory_failure_are_both_retained() {
        let requested_path = PathBuf::from("/requested/history.db");
        let runtime = HistoryRuntime::initialize_with_factories(
            &crate::config::HistoryMode::Persistent { dir: None },
            Some(requested_path.clone()),
            HistoryKind::R,
            None,
            None,
            |_path, _kind, _session, _timestamp| injected_failure("persistent open failure"),
            |_session, _timestamp| injected_failure("fallback memory failure"),
        );

        let detail = runtime.diagnostic_detail().expect("unavailable detail");
        assert_eq!(runtime.requested_path(), Some(requested_path.as_path()));
        let HistoryRuntime::Unavailable {
            failure,
            previous_failure,
        } = runtime
        else {
            panic!("persistent and fallback initialization should be unavailable");
        };
        assert_eq!(failure.stage, HistoryFailureStage::MemoryInitialization);
        assert!(failure.message.contains("fallback memory failure"));
        let previous_failure = previous_failure.expect("persistent failure should be retained");
        assert_eq!(previous_failure.stage, HistoryFailureStage::PersistentOpen);
        assert!(previous_failure.message.contains("persistent open failure"));
        assert!(detail.contains("fallback memory failure"));
        let requested_path_text = requested_path.to_string_lossy();
        assert_eq!(detail.matches(requested_path_text.as_ref()).count(), 0);
        let warning = HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: Some(HistoryFailureDetail::test_persistent_open(
                requested_path.clone(),
            )),
        }
        .startup_warning()
        .expect("unavailable warning");
        assert_eq!(warning.matches(requested_path_text.as_ref()).count(), 1);
    }
}
