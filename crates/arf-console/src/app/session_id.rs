//! Reedline history session ID creation.

use crate::config;
use crate::config::Config;
use reedline::Reedline;

/// Generate a history session ID when history is enabled and a history directory
/// is available, or `None` otherwise.
///
/// This ensures IPC/session JSON does not misleadingly advertise history isolation
/// when no history backend is configured.
pub(crate) fn create_session_id(config: &Config) -> Option<reedline::HistorySessionId> {
    if config.history.disabled {
        return None;
    }
    // Check that a history directory is actually resolvable, matching the logic
    // in Repl::r_history_path() / shell_history_path().
    if config.history.dir.is_none() && config::history_dir().is_none() {
        return None;
    }
    Reedline::create_history_session_id()
}

#[cfg(test)]
mod session_id_tests {
    use super::*;

    #[test]
    fn test_create_session_id_when_history_enabled() {
        let mut config = Config::default();
        // Ensure a history dir is available by setting it explicitly
        config.history.dir = Some(std::env::temp_dir());
        assert!(!config.history.disabled);
        let id = create_session_id(&config);
        assert!(
            id.is_some(),
            "should generate session ID when history is enabled"
        );
    }

    #[test]
    fn test_create_session_id_when_history_disabled() {
        let mut config = Config::default();
        config.history.disabled = true;
        let id = create_session_id(&config);
        assert!(id.is_none(), "should be None when history is disabled");
    }

    #[test]
    fn test_create_session_id_respects_default_history_dir() {
        // create_session_id reads HOME/XDG variables indirectly through history_dir.
        let _guard = crate::test_utils::lock_env();
        // With default config (history.dir = None), session ID depends on
        // whether the platform provides a data directory via history_dir().
        let config = Config::default();
        assert!(!config.history.disabled);
        assert!(config.history.dir.is_none());
        let id = create_session_id(&config);
        // On most platforms history_dir() returns Some, so session ID is generated.
        // On exotic platforms where it returns None, session ID should be None.
        assert_eq!(id.is_some(), config::history_dir().is_some());
    }
}
