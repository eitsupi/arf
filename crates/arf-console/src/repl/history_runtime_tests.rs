use super::*;

#[test]
fn prepared_r_and_shell_histories_are_distinct_stable_owners() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.history.mode = crate::config::HistoryMode::Persistent {
        dir: Some(temp_dir.path().to_path_buf()),
    };
    let repl = Repl::new(
        config,
        None,
        ConfigStatus::Ok,
        RSourceStatus::Path,
        None,
        Reedline::create_history_session_id(),
    )
    .unwrap();
    let (r_runtime, shell_runtime) = repl.initialize_history_runtimes();
    let r_store = r_runtime.store().unwrap();
    let shell_store = shell_runtime.store().unwrap();
    assert!(!r_store.same_owner(&shell_store));
    assert!(shell_runtime.store().unwrap().same_owner(&shell_store));
}

#[test]
fn history_database_paths_share_the_resolved_location() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.history.mode = crate::config::HistoryMode::Persistent {
        dir: Some(temp_dir.path().to_path_buf()),
    };
    let repl = Repl::new(
        config,
        None,
        ConfigStatus::Ok,
        RSourceStatus::Path,
        None,
        Reedline::create_history_session_id(),
    )
    .unwrap();

    assert_eq!(repl.r_history_path(), Some(temp_dir.path().join("r.db")));
    assert_eq!(
        repl.shell_history_path(),
        Some(temp_dir.path().join("shell.db"))
    );
    assert_eq!(
        repl.history_location.source(),
        crate::config::HistoryLocationSource::Explicit
    );
}

#[test]
fn volatile_repl_has_no_persistent_history_paths() {
    let mut config = Config::default();
    config.history.mode = crate::config::HistoryMode::Volatile;
    let repl = Repl::new(
        config,
        None,
        ConfigStatus::Ok,
        RSourceStatus::Path,
        None,
        Reedline::create_history_session_id(),
    )
    .unwrap();

    assert_eq!(repl.r_history_path(), None);
    assert_eq!(repl.shell_history_path(), None);
    assert_eq!(
        repl.history_location.source(),
        crate::config::HistoryLocationSource::Volatile
    );
}
