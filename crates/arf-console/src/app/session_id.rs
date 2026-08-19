//! Reedline history session ID creation.

use crate::config::Config;
use reedline::Reedline;

/// Generate a session ID for both persistent and volatile history.
///
/// Volatile history still needs session isolation while the process is alive;
/// it simply does not use the ID to read or write a file.
pub(crate) fn create_session_id(_config: &Config) -> Option<reedline::HistorySessionId> {
    Reedline::create_history_session_id()
}

#[cfg(test)]
mod session_id_tests {
    use super::*;

    #[test]
    fn test_create_session_id_when_history_enabled() {
        let mut config = Config::default();
        // Ensure a history dir is available by setting it explicitly
        config.history.mode = crate::config::HistoryMode::Persistent {
            dir: Some(std::env::temp_dir()),
        };
        let id = create_session_id(&config);
        assert!(
            id.is_some(),
            "should generate session ID when history is enabled"
        );
    }

    #[test]
    fn test_create_session_id_when_history_volatile() {
        let mut config = Config::default();
        config.history.mode = crate::config::HistoryMode::Volatile;
        let id = create_session_id(&config);
        assert!(id.is_some(), "volatile history still has a session ID");
    }

    #[test]
    fn test_create_session_id_is_independent_of_history_directory() {
        let _guard = crate::test_utils::lock_env();
        // Session IDs are independent of the persistent directory.
        let config = Config::default();
        assert!(matches!(
            config.history.mode,
            crate::config::HistoryMode::Persistent { dir: None }
        ));
        let id = create_session_id(&config);
        // Session IDs are generated independently of the configured directory.
        assert!(id.is_some());
    }
}
