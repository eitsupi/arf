use super::generate_info_lines;
use super::info::history_runtime_label;
use super::info::mask_env_value;
use super::render::{append_ipc_info_lines, style_info_line};
use super::*;
use crate::config::{ConfigStatus, HistoryLocationSource, mask_home_path};
use crate::history::HistoryFailureDetail;
use crate::ipc::policy::{IpcPolicy, SilentPolicy};
use ratatui::style::{Color, Modifier, Style};

#[test]
fn test_mask_env_value_masks_each_path_segment() {
    // Read HOME through dirs::home_dir and mask_env_value -> mask_home_path.
    let _guard = crate::test_utils::lock_env();
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let home_str = home.display().to_string();
    let path_sep = std::path::MAIN_SEPARATOR;
    let list_sep = if cfg!(windows) { ';' } else { ':' };
    let outside = "/opt/R/library";
    // mask_env_value round-trips through Path::display() which may normalize separators
    let outside_expected = std::path::Path::new(outside).display().to_string();
    let test_value = format!(
        "{home_str}{path_sep}.R{path_sep}library{list_sep}{home_str}{path_sep}other{list_sep}{outside}"
    );
    assert_eq!(
        mask_env_value(&test_value),
        format!(
            "~{path_sep}.R{path_sep}library{list_sep}~{path_sep}other{list_sep}{outside_expected}"
        ),
        "home segments should be masked and other segments left unchanged"
    );
}

#[test]
fn test_style_info_line_heading() {
    let styled = style_info_line("# Session Information");
    // Should have bold modifier
    assert!(
        styled.spans[0].style.add_modifier.contains(Modifier::BOLD),
        "Heading should be bold"
    );
}

#[test]
fn test_style_info_line_h2_heading() {
    let styled = style_info_line("## Environment Variables");
    assert!(
        styled.spans[0].style.add_modifier.contains(Modifier::BOLD),
        "H2 heading should be bold"
    );
}

#[test]
fn test_style_info_line_key_value() {
    let styled = style_info_line("arf version:    0.2.1");
    // Key part should be cyan
    assert_eq!(styled.spans[0].style.fg, Some(Color::Cyan));
    assert_eq!(styled.spans[0].content, "arf version");
    // Value part should be unstyled
    assert_eq!(styled.spans[1].content, ":    0.2.1");
}

#[test]
fn test_style_info_line_env_var() {
    let styled = style_info_line("R_LIBS:         /path/to/libs");
    assert_eq!(styled.spans[0].style.fg, Some(Color::Cyan));
    assert_eq!(styled.spans[0].content, "R_LIBS");
}

#[test]
fn test_style_info_line_empty() {
    let styled = style_info_line("");
    // Empty string produces a Line with no spans
    assert!(styled.spans.is_empty() || styled.spans[0].content.is_empty());
}

#[test]
fn test_style_info_line_plain() {
    let styled = style_info_line("Some plain text without special formatting");
    assert_eq!(styled.spans.len(), 1);
    assert_eq!(
        styled.spans[0].content,
        "Some plain text without special formatting"
    );
    assert_eq!(styled.spans[0].style, Style::default());
}

#[test]
fn test_session_info_content_as_plain_text() {
    let lines = vec![
        "# Test".to_string(),
        "key: value".to_string(),
        "".to_string(),
    ];
    let content = session_info_content(lines);
    assert_eq!(content.copy_text(), Some("# Test\nkey: value\n"));
}

// --- generate_info_lines config_path tests ---

use crate::editor::prompt::PromptFormatter;
use crate::repl::state::PromptRuntimeConfig;

fn default_prompt_config() -> PromptRuntimeConfig {
    PromptRuntimeConfig::builder(PromptFormatter::default(), "r> ", "+  ", "[bash] $ ").build()
}

fn default_reprex_runtime() -> ReprexRuntime {
    ReprexRuntime::new(
        crate::config::ReprexMode::Off,
        "#> ",
        crate::config::FormatterBackend::Air,
    )
}

fn unavailable_history() -> HistoryRuntime {
    HistoryRuntime::Unavailable {
        failure: HistoryFailureDetail::test_memory(),
        previous_failure: None,
    }
}

#[test]
fn test_generate_info_lines_config_path_none() {
    // Read SHELL through PromptFormatter::new and R_HOME through generate_info_lines.
    let _guard = crate::test_utils::lock_env();
    let config = default_prompt_config();
    let lines = generate_info_lines(
        &config,
        &default_reprex_runtime(),
        &ConfigFileInfo {
            path: None,
            status: ConfigStatus::Ok,
            source: crate::config::ConfigFileSource::Default,
        },
        &unavailable_history(),
        &unavailable_history(),
        HistoryLocationSource::Default,
        &RSourceStatus::Path,
    );
    let ipc_index = lines.iter().position(|line| line == "## IPC").unwrap();
    let shell_history_index = lines
        .iter()
        .position(|line| line.starts_with("Shell history:"))
        .unwrap();
    assert!(ipc_index > shell_history_index);
    assert_eq!(lines[ipc_index - 1], "");
    assert_ne!(lines[ipc_index - 2], "");
    let config_line = lines
        .iter()
        .find(|l| l.starts_with("Config file:"))
        .unwrap();
    assert!(
        config_line.contains("using defaults"),
        "None path should show defaults: {}",
        config_line
    );
    assert!(lines.iter().any(|line| line == "Config source:  default"));
}

#[test]
fn test_generate_info_lines_config_path_existing() {
    // Read SHELL through PromptFormatter::new and R_HOME through generate_info_lines.
    let _guard = crate::test_utils::lock_env();
    let config = default_prompt_config();
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();
    let masked = mask_home_path(&path);
    let lines = generate_info_lines(
        &config,
        &default_reprex_runtime(),
        &ConfigFileInfo {
            path: Some(path),
            status: ConfigStatus::Ok,
            source: crate::config::ConfigFileSource::CommandLine,
        },
        &unavailable_history(),
        &unavailable_history(),
        HistoryLocationSource::Default,
        &RSourceStatus::Path,
    );
    let config_line = lines
        .iter()
        .find(|l| l.starts_with("Config file:"))
        .unwrap();
    assert!(
        config_line.contains(&masked),
        "Existing path should contain the file path: {}",
        config_line
    );
    assert!(
        !config_line.contains("not found"),
        "Existing path should not say 'not found': {}",
        config_line
    );
    assert!(
        !config_line.contains("using defaults"),
        "Existing path should not say 'using defaults': {}",
        config_line
    );
    assert!(lines.iter().any(|line| line == "Config source:  --config"));
}

#[test]
fn test_generate_info_lines_config_path_nonexistent() {
    // Read SHELL through PromptFormatter::new and R_HOME through generate_info_lines.
    let _guard = crate::test_utils::lock_env();
    let config = default_prompt_config();
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("nonexistent_config.toml");
    let lines = generate_info_lines(
        &config,
        &default_reprex_runtime(),
        &ConfigFileInfo {
            path: Some(path),
            status: ConfigStatus::Ok,
            source: crate::config::ConfigFileSource::Default,
        },
        &unavailable_history(),
        &unavailable_history(),
        HistoryLocationSource::Default,
        &RSourceStatus::Path,
    );
    let config_line = lines
        .iter()
        .find(|l| l.starts_with("Config file:"))
        .unwrap();
    assert!(
        config_line.contains("not found"),
        "Non-existing path should say 'not found': {}",
        config_line
    );
}

#[test]
fn test_generate_info_lines_config_parse_error() {
    // Read SHELL through PromptFormatter::new and R_HOME through generate_info_lines.
    let _guard = crate::test_utils::lock_env();
    let config = default_prompt_config();
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();
    let lines = generate_info_lines(
        &config,
        &default_reprex_runtime(),
        &ConfigFileInfo {
            path: Some(path),
            status: ConfigStatus::ParseError,
            source: crate::config::ConfigFileSource::Default,
        },
        &unavailable_history(),
        &unavailable_history(),
        HistoryLocationSource::Default,
        &RSourceStatus::Path,
    );
    let config_line = lines
        .iter()
        .find(|l| l.starts_with("Config file:"))
        .unwrap();
    assert!(
        config_line.contains("parse error"),
        "Parse error should be shown: {}",
        config_line
    );
}

#[test]
fn test_generate_info_lines_config_read_error() {
    // Read SHELL through PromptFormatter::new and R_HOME through generate_info_lines.
    let _guard = crate::test_utils::lock_env();
    let config = default_prompt_config();
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();
    let lines = generate_info_lines(
        &config,
        &default_reprex_runtime(),
        &ConfigFileInfo {
            path: Some(path),
            status: ConfigStatus::ReadError,
            source: crate::config::ConfigFileSource::Default,
        },
        &unavailable_history(),
        &unavailable_history(),
        HistoryLocationSource::Default,
        &RSourceStatus::Path,
    );
    let config_line = lines
        .iter()
        .find(|l| l.starts_with("Config file:"))
        .unwrap();
    assert!(
        config_line.contains("read error"),
        "Read error should be shown: {}",
        config_line
    );
}

#[test]
fn history_runtime_labels_cover_all_lifecycle_states() {
    use crate::history::{
        HistoryFailureDetail, HistoryHandle, HistorySaveReceipt, HistoryStore,
        VolatileHistoryReason,
    };

    let store = HistoryStore::in_memory(None, None).unwrap();
    let handle = || HistoryHandle {
        store: store.clone(),
        receipt: HistorySaveReceipt::new(),
    };
    let configured = HistoryRuntime::Volatile {
        handle: handle(),
        reason: VolatileHistoryReason::Configured,
    };
    let fallback_with_path = HistoryRuntime::Volatile {
        handle: handle(),
        reason: VolatileHistoryReason::Fallback {
            persistent_failure: HistoryFailureDetail::test_persistent_open(
                "/tmp/history.db".into(),
            ),
        },
    };
    let fallback_without_path = HistoryRuntime::Volatile {
        handle: handle(),
        reason: VolatileHistoryReason::Fallback {
            persistent_failure: HistoryFailureDetail::test_path_resolution(),
        },
    };
    let unavailable = HistoryRuntime::Unavailable {
        failure: HistoryFailureDetail::test_memory(),
        previous_failure: None,
    };
    let persistent_dir = tempfile::tempdir().unwrap();
    let persistent_path = persistent_dir.path().join("history.db");
    let persistent = HistoryRuntime::Persistent(HistoryHandle {
        store: HistoryStore::open(
            persistent_path.clone(),
            crate::history::artifact::HistoryKind::R,
            None,
            None,
        )
        .unwrap(),
        receipt: HistorySaveReceipt::new(),
    });

    assert!(
        history_runtime_label(&persistent, HistoryLocationSource::Default).contains("persistent (")
    );
    assert!(
        history_runtime_label(&persistent, HistoryLocationSource::LegacyDefault)
            .contains("legacy default location")
    );
    assert!(
        history_runtime_label(&configured, HistoryLocationSource::Volatile)
            .contains("volatile (session only)")
    );
    assert!(
        history_runtime_label(&fallback_with_path, HistoryLocationSource::LegacyDefault)
            .contains("fallback; requested path: /tmp/history.db; legacy default location)")
    );
    let unavailable_with_path = HistoryRuntime::Unavailable {
        failure: HistoryFailureDetail::test_memory(),
        previous_failure: Some(HistoryFailureDetail::test_persistent_open(
            "/tmp/history.db".into(),
        )),
    };
    assert!(
        history_runtime_label(&unavailable_with_path, HistoryLocationSource::Explicit)
            .starts_with("unavailable (requested path: /tmp/history.db; explicit directory)")
    );
    assert!(
        history_runtime_label(&fallback_without_path, HistoryLocationSource::Default)
            .contains("fallback; no persistent path; default location)")
    );
    assert!(
        history_runtime_label(&unavailable, HistoryLocationSource::Explicit)
            .starts_with("unavailable (explicit directory)")
    );
}

#[test]
fn ipc_info_shows_server_state_and_policy() {
    let mut lines = Vec::new();
    append_ipc_info_lines(
        &mut lines,
        &IpcPolicy {
            silent: SilentPolicy::Restricted {
                allowed_functions: vec!["+".to_string(), "stats::median".to_string()],
            },
            visible: crate::ipc::policy::VisiblePolicy::ApprovalRequired,
        },
        true,
    );

    insta::assert_snapshot!(lines.join("\n"), @r###"
## IPC

Server: enabled
Silent eval:    restricted
Allowed functions:
  +
  stats::median
Visible requests: approval required
"###);

    let mut unrestricted_lines = Vec::new();
    append_ipc_info_lines(
        &mut unrestricted_lines,
        &IpcPolicy {
            silent: SilentPolicy::Unrestricted,
            visible: crate::ipc::policy::VisiblePolicy::ApprovalNotRequired,
        },
        true,
    );
    insta::assert_snapshot!(unrestricted_lines.join("\n"), @r###"
## IPC

Server: enabled
Silent eval:    unrestricted
Visible requests: approval not required
"###);

    let mut disabled_lines = Vec::new();
    append_ipc_info_lines(
        &mut disabled_lines,
        &IpcPolicy {
            silent: SilentPolicy::Unrestricted,
            visible: crate::ipc::policy::VisiblePolicy::ApprovalNotRequired,
        },
        false,
    );
    insta::assert_snapshot!(disabled_lines.join("\n"), @r###"
## IPC

Server: disabled
"###);
}
