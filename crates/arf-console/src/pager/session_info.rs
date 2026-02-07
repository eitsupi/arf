//! Session information display.

use super::{PagerAction, PagerConfig, PagerContent, copy_to_clipboard, run};
use crate::config::{RSourceStatus, mask_home_path};
use crate::editor::prompt::get_r_version;
use crate::external::{formatter, rig};
use crate::repl::state::PromptRuntimeConfig;

use crossterm::event::{KeyCode, KeyModifiers};
use std::path::PathBuf;

/// Display session information for the :info command in a pager.
pub fn display_session_info(
    prompt_config: &PromptRuntimeConfig,
    config_path: &Option<PathBuf>,
    r_history_path: &Option<PathBuf>,
    shell_history_path: &Option<PathBuf>,
    r_source_status: &RSourceStatus,
) {
    let lines = generate_info_lines(
        prompt_config,
        config_path,
        r_history_path,
        shell_history_path,
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
    config_path: &Option<PathBuf>,
    r_history_path: &Option<PathBuf>,
    shell_history_path: &Option<PathBuf>,
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
            lines.push(format!("Config file:    {}", mask_home_path(path)));
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
    if rig::rig_available() {
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
    } else {
        lines.push("rig:            not installed".to_string());
    }

    // Air (formatter) status
    if formatter::is_formatter_available() {
        lines.push("air:            installed".to_string());
    } else {
        lines.push("air:            not installed".to_string());
    }

    lines.push(String::new());

    // Current mode
    let mode = if prompt_config.is_shell_enabled() {
        "Shell"
    } else if prompt_config.is_reprex_enabled() {
        "R (reprex)"
    } else {
        "R"
    };
    lines.push(format!("Current mode:   {}", mode));

    // Autoformat status (only relevant in reprex mode)
    if prompt_config.is_reprex_enabled() {
        let autoformat = if prompt_config.is_autoformat_enabled() {
            "enabled"
        } else {
            "disabled"
        };
        lines.push(format!("Auto-format:    {}", autoformat));
    }

    lines.push(String::new());

    // History paths
    if let Some(path) = r_history_path {
        lines.push(format!("R history:      {}", mask_home_path(path)));
    }
    if let Some(path) = shell_history_path {
        lines.push(format!("Shell history:  {}", mask_home_path(path)));
    }

    lines.push(String::new());

    // R-related environment variables
    // Format: "VAR_NAME:       value" with aligned colons
    let env_vars = [
        ("R_LIBS", "R_LIBS:         "),
        ("R_LIBS_USER", "R_LIBS_USER:    "),
        ("R_LIBS_SITE", "R_LIBS_SITE:    "),
        ("R_PROFILE", "R_PROFILE:      "),
        ("R_ENVIRON", "R_ENVIRON:      "),
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

    lines
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

    fn render_line(&self, index: usize, _width: usize) -> String {
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

/// Apply styling to an info line.
fn style_info_line(line: &str) -> String {
    use crossterm::style::Stylize;

    // Headings (# and ##)
    if line.starts_with("# ") || line.starts_with("## ") {
        return line.bold().to_string();
    }

    // Key-value pairs (including environment variables)
    if let Some(colon_idx) = line.find(':')
        && !line.starts_with(' ')
    {
        let (key, value) = line.split_at(colon_idx);
        // Style the key part, keep value plain
        return format!("{}{}", key.cyan(), value);
    }

    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_env_value_with_home() {
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
        let test_value = "/opt/R/library";
        // mask_env_value round-trips through Path::display() which may normalize separators
        let expected = std::path::Path::new(test_value).display().to_string();
        let masked = mask_env_value(test_value);
        assert_eq!(masked, expected, "Should not change non-home paths");
    }

    #[test]
    fn test_mask_env_value_multiple_paths() {
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
        let line = "# Session Information";
        let styled = style_info_line(line);
        // Should contain ANSI codes for bold
        assert!(styled.contains("\x1b"), "Heading should be styled");
    }

    #[test]
    fn test_style_info_line_h2_heading() {
        let line = "## Environment Variables";
        let styled = style_info_line(line);
        // Should contain ANSI codes for bold
        assert!(styled.contains("\x1b"), "H2 heading should be styled");
    }

    #[test]
    fn test_style_info_line_key_value() {
        let line = "arf version:    0.2.1";
        let styled = style_info_line(line);
        // Should contain ANSI codes for cyan key
        assert!(styled.contains("\x1b"), "Key-value should be styled");
    }

    #[test]
    fn test_style_info_line_env_var() {
        // Environment variables now use key: value format like other lines
        let line = "R_LIBS:         /path/to/libs";
        let styled = style_info_line(line);
        // Should contain ANSI codes for cyan key
        assert!(styled.contains("\x1b"), "Env var should be styled");
    }

    #[test]
    fn test_style_info_line_empty() {
        let line = "";
        let styled = style_info_line(line);
        assert_eq!(styled, "", "Empty line should remain empty");
    }

    #[test]
    fn test_style_info_line_plain() {
        let line = "Some plain text without special formatting";
        let styled = style_info_line(line);
        // Plain text without : or = at special positions should not be styled
        assert_eq!(styled, line);
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
}
