//! Session information display.

use super::{PagerAction, PagerConfig, PagerContent, copy_to_clipboard, run};
use crate::config::{ConfigStatus, RSourceStatus, mask_home_path};
use crate::editor::prompt::get_r_version;
use crate::external::rig;
use crate::history::HistoryRuntime;
use crate::ipc::policy::{IpcPolicy, SilentPolicy, VisiblePolicy, policy};
use crate::ipc::session::SessionType;
use crate::repl::reprex::ReprexRuntime;
use crate::repl::state::PromptRuntimeConfig;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::PathBuf;

/// Display session information for the :info command in a pager.
pub fn display_session_info(
    prompt_config: &PromptRuntimeConfig,
    reprex: &ReprexRuntime,
    config_path: &Option<PathBuf>,
    config_status: ConfigStatus,
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    r_source_status: &RSourceStatus,
) {
    let lines = generate_info_lines(
        prompt_config,
        reprex,
        config_path,
        config_status,
        r_history,
        shell_history,
        r_source_status,
    );

    let mut content = SessionInfoContent::new(lines);

    let config = PagerConfig {
        title: "Session Info",
        footer_hint: "↑↓/jk scroll │ c copy │ q exit",
        manage_alternate_screen: true,
    };

    if let Err(e) = run(&mut content, &config) {
        eprintln!("Pager error: {}", e);
    }
}

/// Generate the session information as a vector of lines.
fn generate_info_lines(
    prompt_config: &PromptRuntimeConfig,
    reprex: &ReprexRuntime,
    config_path: &Option<PathBuf>,
    config_status: ConfigStatus,
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    r_source_status: &RSourceStatus,
) -> Vec<String> {
    let mut lines = Vec::new();

    lines.push("# Session Information".to_string());
    lines.push(String::new());

    // arf version
    lines.push(format!("arf version:    {}", env!("CARGO_PKG_VERSION")));

    // OS information
    lines.push(format!(
        "OS:             {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));

    // Config file path
    if let Some(path) = config_path {
        if path.exists() {
            match config_status {
                ConfigStatus::Ok => {
                    lines.push(format!("Config file:    {}", mask_home_path(path)));
                }
                ConfigStatus::ParseError => {
                    lines.push(format!(
                        "Config file:    {} (parse error, using defaults)",
                        mask_home_path(path)
                    ));
                }
                ConfigStatus::ReadError => {
                    lines.push(format!(
                        "Config file:    {} (read error, using defaults)",
                        mask_home_path(path)
                    ));
                }
            }
        } else {
            lines.push(format!(
                "Config file:    {} (not found, using defaults)",
                mask_home_path(path)
            ));
        }
    } else {
        lines.push("Config file:    (using defaults)".to_string());
    }

    // R version
    let r_version = get_r_version();
    if r_version.is_empty() {
        lines.push("R version:      (not available)".to_string());
    } else {
        lines.push(format!("R version:      {}", r_version));
    }

    // R_HOME
    if let Ok(r_home) = std::env::var("R_HOME") {
        let r_home_path = std::path::Path::new(&r_home);
        lines.push(format!("R_HOME:         {}", mask_home_path(r_home_path)));
    }

    // R source (how R was resolved at startup)
    lines.push(format!("R source:       {}", r_source_status.display()));

    lines.push(String::new());

    // rig status
    match rig::rig_available() {
        Ok(()) => {
            let mut rig_line = "rig:            installed".to_string();
            if let Ok(versions) = rig::list_versions()
                && !versions.is_empty()
            {
                let version_list: Vec<_> = versions
                    .iter()
                    .map(|v| {
                        if v.default {
                            format!("{}*", v.name)
                        } else {
                            v.name.clone()
                        }
                    })
                    .collect();
                rig_line.push_str(&format!(" ({})", version_list.join(", ")));
            }
            lines.push(rig_line);
        }
        Err(rig::RigError::NotInstalled) => {
            lines.push("rig:            not installed".to_string());
        }
        Err(rig::RigError::CommandFailed(reason)) => {
            lines.push(format!(
                "rig:            installed but unavailable ({reason})"
            ));
        }
        Err(error) => {
            lines.push(format!("rig:            unavailable ({error})"));
        }
    }

    // Current mode
    let mode = if prompt_config.is_shell_enabled() {
        "Shell"
    } else {
        "R"
    };
    lines.push(format!("Current mode:   {}", mode));

    let reprex_mode = match reprex.mode {
        crate::config::ReprexMode::Off => "off",
        crate::config::ReprexMode::On => "on",
        crate::config::ReprexMode::Format => "format",
    };
    lines.push(format!("Reprex:         {}", reprex_mode));
    let formatter_label = match (reprex.formatter_selector, reprex.formatter) {
        (crate::config::ReprexFormatter::Auto, Some(backend)) => {
            format!("auto ({} installed)", backend.command())
        }
        (crate::config::ReprexFormatter::Auto, None) => "auto (unavailable)".to_string(),
        (selector, Some(backend)) => format!("{selector} ({} installed)", backend.command()),
        (selector, None) => format!("{selector} (missing)"),
    };
    lines.push(format!("Formatter:      {formatter_label}"));

    lines.push(String::new());

    // History lifecycle state.  Volatile runtimes intentionally have no
    // persistent path to display, while fallbacks retain the requested path
    // as a diagnostic without implying that it was opened.
    lines.push(format!(
        "R history:      {}",
        history_runtime_label(r_history)
    ));
    lines.push(format!(
        "Shell history:  {}",
        history_runtime_label(shell_history)
    ));

    lines.push(String::new());

    // R-related environment variables
    // Format: "VAR_NAME:       value" with aligned colons
    let env_vars = [
        ("ARF_HISTORY_DIR", "ARF_HISTORY_DIR: "),
        ("R_LIBS", "R_LIBS:          "),
        ("R_LIBS_USER", "R_LIBS_USER:     "),
        ("R_LIBS_SITE", "R_LIBS_SITE:     "),
        ("R_PROFILE", "R_PROFILE:       "),
        ("R_ENVIRON", "R_ENVIRON:       "),
    ];
    let mut has_env = false;
    for (var, label) in &env_vars {
        if let Ok(value) = std::env::var(var) {
            if !has_env {
                lines.push("## Environment Variables".to_string());
                lines.push(String::new());
                has_env = true;
            }
            // Mask paths in environment variables too
            let masked_value = mask_env_value(&value);
            lines.push(format!("{}{}", label, masked_value));
        }
    }

    append_section_separator(&mut lines);

    // IPC policy. Keep each allowlist target on its own line so a
    // long list remains fully visible in the pager's vertical scroll view.
    append_ipc_info_lines(
        &mut lines,
        &policy(SessionType::Interactive),
        crate::ipc::server::is_server_running(),
    );

    lines
}

fn append_ipc_info_lines(lines: &mut Vec<String>, ipc_policy: &IpcPolicy, server_enabled: bool) {
    lines.push("## IPC".to_string());
    lines.push(String::new());
    lines.push(format!(
        "Server: {}",
        if server_enabled {
            "enabled"
        } else {
            "disabled"
        }
    ));
    if !server_enabled {
        return;
    }

    match &ipc_policy.silent {
        SilentPolicy::Restricted { allowed_functions } => {
            lines.push("Silent eval:    restricted".to_string());
            lines.push("Allowed functions:".to_string());
            if allowed_functions.is_empty() {
                lines.push("  (none; bare literals and identifiers remain allowed)".to_string());
            } else {
                for function in allowed_functions {
                    lines.push(format!("  {function}"));
                }
            }
        }
        SilentPolicy::Unrestricted => {
            lines.push("Silent eval:    unrestricted".to_string());
        }
    }
    lines.push(format!(
        "Visible requests: approval {}",
        visible_approval_label(&ipc_policy.visible)
    ));
}

fn visible_approval_label(policy: &VisiblePolicy) -> &'static str {
    match policy {
        VisiblePolicy::ApprovalRequired => "required",
        VisiblePolicy::ApprovalNotRequired => "not required",
    }
}

fn append_section_separator(lines: &mut Vec<String>) {
    if !matches!(lines.last(), Some(line) if line.is_empty()) {
        lines.push(String::new());
    }
}

fn history_runtime_label(runtime: &HistoryRuntime) -> String {
    let label = match runtime {
        HistoryRuntime::Persistent(_) => runtime
            .requested_path()
            .map(|path| format!("persistent ({})", mask_home_path(path)))
            .unwrap_or_else(|| "persistent".to_string()),
        HistoryRuntime::Volatile { reason, .. } => match reason {
            crate::history::VolatileHistoryReason::Configured => {
                "volatile (session only)".to_string()
            }
            crate::history::VolatileHistoryReason::Fallback { .. } => {
                match runtime.requested_path() {
                    Some(path) => format!(
                        "volatile (fallback; requested path: {})",
                        mask_home_path(path)
                    ),
                    None => "volatile (fallback; no persistent path)".to_string(),
                }
            }
        },
        HistoryRuntime::Unavailable { .. } => match runtime.requested_path() {
            Some(path) => format!("unavailable (requested path: {})", mask_home_path(path)),
            None => "unavailable".to_string(),
        },
    };
    match runtime.diagnostic_detail() {
        Some(detail) if !label.contains(&detail) => format!("{label}; {detail}"),
        _ => label,
    }
}

/// Mask home directory in environment variable value.
///
/// Handles path-like values which may contain multiple paths separated by
/// the platform's path list separator (`:` on Unix, `;` on Windows).
/// Each path segment is individually checked and masked if it starts with
/// the home directory.
fn mask_env_value(value: &str) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };

    let masked_parts: Vec<String> = value
        .split(separator)
        .map(|part| {
            let path = std::path::Path::new(part);
            mask_home_path(path)
        })
        .collect();

    masked_parts.join(&separator.to_string())
}

/// Content wrapper for displaying session info in the pager.
struct SessionInfoContent {
    /// Raw info lines.
    lines: Vec<String>,
    /// Feedback message for user actions.
    feedback_message: Option<String>,
}

impl SessionInfoContent {
    fn new(lines: Vec<String>) -> Self {
        Self {
            lines,
            feedback_message: None,
        }
    }

    /// Get all content as plain text for copying.
    fn as_plain_text(&self) -> String {
        self.lines.join("\n")
    }
}

impl PagerContent for SessionInfoContent {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
        let line = &self.lines[index];
        style_info_line(line)
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<PagerAction> {
        // Copy all content to clipboard
        if code == KeyCode::Char('c') && modifiers == KeyModifiers::NONE {
            let text = self.as_plain_text();
            if copy_to_clipboard(&text).is_ok() {
                self.feedback_message = Some("Copied session info to clipboard".to_string());
            } else {
                self.feedback_message = Some("Failed to copy".to_string());
            }
            return None; // Don't exit, just show feedback
        }
        None
    }

    fn feedback_message(&self) -> Option<&str> {
        self.feedback_message.as_deref()
    }

    fn clear_feedback(&mut self) {
        self.feedback_message = None;
    }
}

/// Apply styling to an info line, returning a ratatui `Line`.
fn style_info_line(line: &str) -> Line<'static> {
    // Headings (# and ##)
    if line.starts_with("# ") || line.starts_with("## ") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }

    // Key-value pairs (including environment variables)
    if let Some(colon_idx) = line.find(':')
        && !line.starts_with(' ')
    {
        let (key, value) = line.split_at(colon_idx);
        return Line::from(vec![
            Span::styled(key.to_string(), Style::default().fg(Color::Cyan)),
            Span::raw(value.to_string()),
        ]);
    }

    Line::from(line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::HistoryFailureDetail;

    #[test]
    fn test_mask_env_value_with_home() {
        // Read HOME through dirs::home_dir and mask_env_value -> mask_home_path.
        let _guard = crate::test_utils::lock_env();
        if let Some(home) = dirs::home_dir() {
            let home_str = home.display().to_string();
            let sep = std::path::MAIN_SEPARATOR;
            let test_value = format!("{}{}R{}library", home_str, sep, sep);
            let masked = mask_env_value(&test_value);
            assert!(masked.starts_with("~"), "Should mask home dir: {}", masked);
            // Check that "R" and "library" are in the path (separator-agnostic)
            assert!(masked.contains("R"), "Should contain R: {}", masked);
            assert!(
                masked.contains("library"),
                "Should contain library: {}",
                masked
            );
        }
    }

    #[test]
    fn test_mask_env_value_without_home() {
        // mask_env_value reads HOME indirectly through mask_home_path.
        let _guard = crate::test_utils::lock_env();
        let test_value = "/opt/R/library";
        // mask_env_value round-trips through Path::display() which may normalize separators
        let expected = std::path::Path::new(test_value).display().to_string();
        let masked = mask_env_value(test_value);
        assert_eq!(masked, expected, "Should not change non-home paths");
    }

    #[test]
    fn test_mask_env_value_multiple_paths() {
        // Read HOME through dirs::home_dir and mask_env_value -> mask_home_path.
        let _guard = crate::test_utils::lock_env();
        if let Some(home) = dirs::home_dir() {
            let home_str = home.display().to_string();
            let path_sep = std::path::MAIN_SEPARATOR;
            let list_sep = if cfg!(windows) { ';' } else { ':' };
            // Platform-appropriate path list
            let test_value = format!(
                "{}{}.R{}library{}{}{}other",
                home_str, path_sep, path_sep, list_sep, home_str, path_sep
            );
            let masked = mask_env_value(&test_value);
            // Both occurrences should be masked
            assert!(
                !masked.contains(&home_str),
                "All home dirs should be masked: {}",
                masked
            );
            // Check that masked output contains ~ prefix
            assert!(masked.starts_with("~"), "Should start with ~: {}", masked);
        }
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
        let content = SessionInfoContent::new(lines);
        let plain = content.as_plain_text();
        assert_eq!(plain, "# Test\nkey: value\n");
    }

    #[test]
    fn test_session_info_content_line_count() {
        let lines = vec!["line1".to_string(), "line2".to_string()];
        let content = SessionInfoContent::new(lines);
        assert_eq!(content.line_count(), 2);
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
            &None,
            ConfigStatus::Ok,
            &unavailable_history(),
            &unavailable_history(),
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
            &Some(path),
            ConfigStatus::Ok,
            &unavailable_history(),
            &unavailable_history(),
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
            &Some(path),
            ConfigStatus::Ok,
            &unavailable_history(),
            &unavailable_history(),
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
            &Some(path),
            ConfigStatus::ParseError,
            &unavailable_history(),
            &unavailable_history(),
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
            &Some(path),
            ConfigStatus::ReadError,
            &unavailable_history(),
            &unavailable_history(),
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
            store: HistoryStore::open(persistent_path.clone(), None, None).unwrap(),
            receipt: HistorySaveReceipt::new(),
        });

        assert!(history_runtime_label(&persistent).contains("persistent ("));
        assert!(history_runtime_label(&configured).contains("volatile (session only)"));
        assert!(history_runtime_label(&fallback_with_path).contains("fallback; requested path"));
        assert!(
            history_runtime_label(&fallback_without_path).contains("fallback; no persistent path")
        );
        assert!(history_runtime_label(&unavailable).starts_with("unavailable"));
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
}
