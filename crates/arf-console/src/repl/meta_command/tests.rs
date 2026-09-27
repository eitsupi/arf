use super::*;
use crate::editor::prompt::PromptFormatter;
use crate::history::HistoryFailureDetail;
use crate::repl::reprex::ReprexRuntime;

fn create_test_prompt_config() -> PromptRuntimeConfig {
    PromptRuntimeConfig::builder(PromptFormatter::default(), "r> ", "+  ", "[bash] $ ").build()
}

/// Default r_source_status for tests (PATH mode, rig not enabled).
fn default_r_source_status() -> RSourceStatus {
    RSourceStatus::Path
}

/// Helper to call process_meta_command with default dir_stack.
fn call_meta(
    input: &str,
    config: &mut PromptRuntimeConfig,
    _r_history_path: &Option<PathBuf>,
    _shell_history_path: &Option<PathBuf>,
    status: &RSourceStatus,
) -> Option<MetaCommandResult> {
    let mut dir_stack = Vec::new();
    let mut reprex =
        ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air);
    process_meta_command(
        input,
        config,
        &mut reprex,
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        status,
        &mut dir_stack,
        None,
        None,
    )
}

fn call_meta_with_runtime(
    input: &str,
    config: &mut PromptRuntimeConfig,
    reprex: &mut ReprexRuntime,
    status: &RSourceStatus,
) -> Option<MetaCommandResult> {
    let mut dir_stack = Vec::new();
    process_meta_command(
        input,
        config,
        reprex,
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        status,
        &mut dir_stack,
        None,
        None,
    )
}

#[test]
fn test_process_meta_command_not_meta() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta("print(x)", &mut config, &None, &None, &status);
    assert!(result.is_none());
}

#[test]
fn test_process_meta_command_reprex_explicit_mode() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let mut reprex =
        ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air);
    let status = default_r_source_status();
    assert!(!reprex.is_enabled());

    let result = call_meta_with_runtime(":reprex on", &mut config, &mut reprex, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(reprex.is_enabled());

    let result = call_meta_with_runtime(":reprex off", &mut config, &mut reprex, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(!reprex.is_enabled());

    let result = call_meta_with_runtime(":reprex", &mut config, &mut reprex, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(!reprex.is_enabled());

    // Once already in format mode, repeating the command is idempotent
    // and does not require probing the formatter again.
    reprex.set_mode(ReprexMode::Format);
    let result = call_meta_with_runtime(":reprex format", &mut config, &mut reprex, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert_eq!(reprex.mode, ReprexMode::Format);

    let result = call_meta_with_runtime(":reprex on extra", &mut config, &mut reprex, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(reprex.is_enabled());
}

#[test]
fn test_process_meta_command_commands() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta(":commands", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));

    // Test alias
    let result = call_meta(":cmds", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
}

#[test]
fn test_process_meta_command_info() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta(":info", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::ShowSessionInfo)));

    // Test alias
    let result = call_meta(":session", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::ShowSessionInfo)));
}

#[test]
fn test_process_meta_command_quit() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta(":quit", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Exit)));

    let result = call_meta(":exit", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Exit)));
}

#[test]
fn test_process_meta_command_unknown() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta(":unknown", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Unknown(_))));
}

#[test]
fn test_process_meta_command_empty_colon() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta(":", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
}

#[test]
fn test_process_meta_command_with_whitespace() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta("  :reprex  ", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    // Bare commands do not change the independent runtime state.
}

#[test]
fn test_process_meta_command_shell_enter() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    assert!(!config.is_shell_enabled());

    let result = call_meta(":shell", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(config.is_shell_enabled());
}

#[test]
fn test_process_meta_command_shell_exit_with_r() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    config.set_shell(true);
    assert!(config.is_shell_enabled());

    let result = call_meta(":r", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(!config.is_shell_enabled());
}

#[test]
fn test_process_meta_command_shell_exit_with_uppercase_r() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    config.set_shell(true);
    assert!(config.is_shell_enabled());

    let result = call_meta(":R", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(!config.is_shell_enabled());
}

#[test]
fn test_process_meta_command_r_when_not_in_shell() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    assert!(!config.is_shell_enabled());

    let result = call_meta(":r", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(!config.is_shell_enabled()); // Still not in shell
}

#[test]
fn test_process_meta_command_system() {
    // create_test_prompt_config and execute_shell_command read SHELL indirectly.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta(":system echo hello", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::ShellExecuted)));
}

#[test]
fn test_process_meta_command_system_empty() {
    // create_test_prompt_config and execute_shell_command read SHELL indirectly.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta(":system", &mut config, &None, &None, &status);
    // Empty :system should still be handled
    assert!(matches!(result, Some(MetaCommandResult::ShellExecuted)));
}

#[test]
fn test_process_meta_command_switch_requires_rig() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();

    // With PATH mode (rig not enabled), :switch should show error
    let status_path = RSourceStatus::Path;
    let result = call_meta(":switch 4.4", &mut config, &None, &None, &status_path);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));

    // With Rig mode (rig enabled), :switch should work (but needs confirmation which we can't test here)
    // Just verify it doesn't immediately reject
    let status_rig = RSourceStatus::Rig {
        version: "4.4.0".to_string(),
        override_info: None,
    };
    // Note: This will prompt for confirmation, so we can't fully test it in unit tests
    // Just testing the setup path here
    let result = call_meta(":switch", &mut config, &None, &None, &status_rig);
    // Without version argument, it should show usage
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
}

// --- cd/pushd/popd tests ---

#[test]
fn test_meta_cd_relative_path() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();
    let subdir = tmp.path().join("sub");
    std::fs::create_dir(&subdir).unwrap();

    std::env::set_current_dir(tmp.path()).unwrap();
    let result = meta_cd("sub");

    assert!(result.is_ok());
    assert!(result.unwrap().ends_with("sub"));
}

#[test]
fn test_meta_cd_absolute_path() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();

    let result = meta_cd(&tmp.path().to_string_lossy());

    assert!(result.is_ok());
}

#[test]
fn test_meta_cd_tilde() {
    // meta_cd reads HOME through dirs::home_dir and changes the cwd.
    let _guard = crate::test_utils::lock_env_and_cwd();
    let result = meta_cd("~");

    assert!(result.is_ok());
    if let Some(home) = dirs::home_dir() {
        assert_eq!(
            result.unwrap().canonicalize().ok(),
            home.canonicalize().ok()
        );
    }
}

#[test]
fn test_meta_cd_no_args() {
    // meta_cd reads HOME through dirs::home_dir and changes the cwd.
    let _guard = crate::test_utils::lock_env_and_cwd();
    let result = meta_cd("");

    assert!(result.is_ok());
    // Should go to home
    if let Some(home) = dirs::home_dir() {
        assert_eq!(
            result.unwrap().canonicalize().ok(),
            home.canonicalize().ok()
        );
    }
}

#[test]
fn test_meta_cd_nonexistent() {
    let result = meta_cd("/nonexistent_path_12345");
    assert!(result.is_err());
}

#[test]
fn test_meta_pushd_popd() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();
    let mut dir_stack = Vec::new();

    std::env::set_current_dir(tmp.path()).unwrap();
    let subdir = tmp.path().join("sub");
    std::fs::create_dir(&subdir).unwrap();

    // pushd into sub
    let result = meta_pushd(&mut dir_stack, "sub");
    assert!(result.is_ok());
    assert_eq!(dir_stack.len(), 1);
    assert!(std::env::current_dir().unwrap().ends_with("sub"));

    // popd back
    let result = meta_popd(&mut dir_stack);
    assert!(result.is_ok());
    assert!(dir_stack.is_empty());
    assert_eq!(
        std::env::current_dir().unwrap().canonicalize().ok(),
        tmp.path().canonicalize().ok()
    );
}

#[test]
fn test_meta_popd_empty_stack() {
    let mut dir_stack: Vec<PathBuf> = Vec::new();
    let result = meta_popd(&mut dir_stack);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty"));
}

#[test]
fn test_meta_pushd_no_args_returns_error() {
    let mut dir_stack: Vec<PathBuf> = Vec::new();
    let result = meta_pushd(&mut dir_stack, "");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Usage"));
    // Stack should not be modified
    assert!(dir_stack.is_empty());
}

#[test]
fn test_meta_pushd_saves_previous() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();
    let subdir = tmp.path().join("sub");
    std::fs::create_dir(&subdir).unwrap();
    let mut dir_stack = Vec::new();

    std::env::set_current_dir(tmp.path()).unwrap();
    let before_pushd = std::env::current_dir().unwrap();

    let _ = meta_pushd(&mut dir_stack, "sub");
    assert_eq!(dir_stack.len(), 1);
    assert_eq!(
        dir_stack[0].canonicalize().ok(),
        before_pushd.canonicalize().ok()
    );
}

#[test]
fn test_process_meta_command_cd() {
    // create_test_prompt_config reads SHELL; this test also changes the cwd.
    let _guard = crate::test_utils::lock_env_and_cwd();
    let tmp = tempfile::tempdir().unwrap();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let mut dir_stack = Vec::new();

    let result = process_meta_command(
        &format!(":cd {}", tmp.path().display()),
        &mut config,
        &mut ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air),
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &status,
        &mut dir_stack,
        None,
        None,
    );
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
}

#[test]
fn test_process_meta_command_pushd_popd() {
    // create_test_prompt_config reads SHELL; this test also changes the cwd.
    let _guard = crate::test_utils::lock_env_and_cwd();
    let tmp = tempfile::tempdir().unwrap();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let mut dir_stack = Vec::new();

    let result = process_meta_command(
        &format!(":pushd {}", tmp.path().display()),
        &mut config,
        &mut ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air),
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &status,
        &mut dir_stack,
        None,
        None,
    );
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert_eq!(dir_stack.len(), 1);

    let result = process_meta_command(
        ":popd",
        &mut config,
        &mut ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air),
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &HistoryRuntime::Unavailable {
            failure: HistoryFailureDetail::test_memory(),
            previous_failure: None,
        },
        &status,
        &mut dir_stack,
        None,
        None,
    );
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
    assert!(dir_stack.is_empty());
}

// --- dir_command_hint tests ---

#[test]
fn test_dir_command_hint_cd() {
    assert!(dir_command_hint("cd /tmp").unwrap().contains(":cd"));
}

#[test]
fn test_dir_command_hint_pushd() {
    assert!(dir_command_hint("pushd /tmp").unwrap().contains(":pushd"));
}

#[test]
fn test_dir_command_hint_popd() {
    assert!(dir_command_hint("popd").unwrap().contains(":popd"));
}

#[test]
fn test_dir_command_hint_other() {
    assert!(dir_command_hint("ls -la").is_none());
    assert!(dir_command_hint("echo cd").is_none());
}

#[test]
fn test_dir_command_hint_empty() {
    assert!(dir_command_hint("").is_none());
}

// --- Force (!) option tests ---

#[test]
fn test_process_meta_command_restart_force() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    // :restart! should skip confirmation and return Restart directly
    let result = call_meta(":restart!", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Restart(None))));
}

#[test]
fn test_process_meta_command_restart_force_with_whitespace() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    let result = call_meta("  :restart!  ", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Restart(None))));
}

#[test]
fn test_process_meta_command_switch_force() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status_rig = RSourceStatus::Rig {
        version: "4.4.0".to_string(),
        override_info: None,
    };
    let result = call_meta(":switch! 4.4", &mut config, &None, &None, &status_rig);
    assert!(matches!(result, Some(MetaCommandResult::Restart(Some(ref v))) if v == "4.4"));
}

#[test]
fn test_process_meta_command_switch_force_accepts_space_after_colon() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status_rig = RSourceStatus::Rig {
        version: "4.4.0".to_string(),
        override_info: None,
    };

    let result = call_meta(": switch! 4.4", &mut config, &None, &None, &status_rig);

    assert!(matches!(
        result,
        Some(MetaCommandResult::Restart(Some(ref version))) if version == "4.4"
    ));
}

#[test]
fn test_process_meta_command_switch_force_accepts_spaced_version_range() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status_rig = RSourceStatus::Rig {
        version: "4.4.0".to_string(),
        override_info: None,
    };

    let result = call_meta(
        ":switch! >=4.3, <5.0",
        &mut config,
        &None,
        &None,
        &status_rig,
    );

    assert!(matches!(
        result,
        Some(MetaCommandResult::Restart(Some(ref version)))
            if version == ">=4.3, <5.0"
    ));
}

#[test]
fn test_process_meta_command_switch_force_preserves_named_version() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status_rig = RSourceStatus::Rig {
        version: "4.4.0".to_string(),
        override_info: None,
    };

    let result = call_meta(":switch! release", &mut config, &None, &None, &status_rig);

    assert!(matches!(
        result,
        Some(MetaCommandResult::Restart(Some(ref version))) if version == "release"
    ));
}

#[test]
fn test_process_meta_command_switch_force_no_version() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status_rig = RSourceStatus::Rig {
        version: "4.4.0".to_string(),
        override_info: None,
    };
    // :switch! without version should still show usage
    let result = call_meta(":switch!", &mut config, &None, &None, &status_rig);
    assert!(matches!(result, Some(MetaCommandResult::Handled)));
}

#[test]
fn test_process_meta_command_unknown_with_bang() {
    // create_test_prompt_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_prompt_config();
    let status = default_r_source_status();
    // :shell! is not a valid command (! only supported on restart/switch)
    let result = call_meta(":shell!", &mut config, &None, &None, &status);
    assert!(matches!(result, Some(MetaCommandResult::Unknown(_))));
}

#[test]
fn history_clear_deduplicates_shared_store_owners() {
    let dir = tempfile::tempdir().unwrap();
    let store = HistoryStore::open(
        dir.path().join("history.db"),
        crate::history::artifact::HistoryKind::R,
        None,
        None,
    )
    .unwrap();
    let unique = HistoryStore::open(
        dir.path().join("other.db"),
        crate::history::artifact::HistoryKind::R,
        None,
        None,
    )
    .unwrap();
    let stores = dedup_history_stores(vec![("R", store.clone()), ("Shell", store), ("R", unique)]);
    assert_eq!(stores.len(), 2);
    assert_eq!(stores[0].0, "R");
}
