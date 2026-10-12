//! Rendering of collected session facts as text lines and styled terminal lines.

use super::info::{ConfigFileState, RigState, SessionInfo};
use crate::config::{ConfigStatus, ReprexFormatter, ReprexMode};
use crate::ipc::policy::{IpcPolicy, SilentPolicy, VisiblePolicy};
use crate::pager::lines_content::LineStyler;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

impl SessionInfo {
    /// Render the session information as a vector of lines.
    pub fn to_lines(&self) -> Vec<String> {
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
        match &self.config_file {
            ConfigFileState::Loaded { path, status } => match status {
                ConfigStatus::Ok => {
                    lines.push(format!("Config file:    {path}"));
                }
                ConfigStatus::ParseError => {
                    lines.push(format!(
                        "Config file:    {path} (parse error, using defaults)"
                    ));
                }
                ConfigStatus::ReadError => {
                    lines.push(format!(
                        "Config file:    {path} (read error, using defaults)"
                    ));
                }
            },
            ConfigFileState::NotFound { path } => {
                lines.push(format!(
                    "Config file:    {path} (not found, using defaults)"
                ));
            }
            ConfigFileState::Defaults => {
                lines.push("Config file:    (using defaults)".to_string());
            }
        }
        lines.push(format!("Config source:  {}", self.config_source));

        // R version
        if self.r_version.is_empty() {
            lines.push("R version:      (not available)".to_string());
        } else {
            lines.push(format!("R version:      {}", self.r_version));
        }

        // R_HOME
        if let Some(r_home) = &self.r_home {
            lines.push(format!("R_HOME:         {r_home}"));
        }

        // R source (how R was resolved at startup)
        lines.push(format!("R source:       {}", self.r_source));

        lines.push(String::new());

        // rig status
        match &self.rig {
            RigState::Installed { versions } => {
                let mut rig_line = "rig:            installed".to_string();
                if !versions.is_empty() {
                    let version_list: Vec<_> = versions
                        .iter()
                        .map(|(name, default)| {
                            if *default {
                                format!("{name}*")
                            } else {
                                name.clone()
                            }
                        })
                        .collect();
                    rig_line.push_str(&format!(" ({})", version_list.join(", ")));
                }
                lines.push(rig_line);
            }
            RigState::NotInstalled => {
                lines.push("rig:            not installed".to_string());
            }
            RigState::CommandFailed(reason) => {
                lines.push(format!(
                    "rig:            installed but unavailable ({reason})"
                ));
            }
            RigState::Unavailable(error) => {
                lines.push(format!("rig:            unavailable ({error})"));
            }
        }

        // Current mode
        let mode = if self.shell_mode { "Shell" } else { "R" };
        lines.push(format!("Current mode:   {}", mode));

        let reprex_mode = match self.reprex_mode {
            ReprexMode::Off => "off",
            ReprexMode::On => "on",
            ReprexMode::Format => "format",
        };
        lines.push(format!("Reprex:         {}", reprex_mode));
        let formatter_label = match (self.formatter_selector, self.formatter_command) {
            (ReprexFormatter::Auto, Some(command)) => format!("auto ({command} installed)"),
            (ReprexFormatter::Auto, None) => "auto (unavailable)".to_string(),
            (selector, Some(command)) => format!("{selector} ({command} installed)"),
            (selector, None) => format!("{selector} (missing)"),
        };
        lines.push(format!("Formatter:      {formatter_label}"));

        lines.push(String::new());

        // History lifecycle state.  Volatile runtimes intentionally have no
        // persistent path to display, while fallbacks retain the requested path
        // as a diagnostic without implying that it was opened.
        lines.push(format!("R history:      {}", self.r_history));
        lines.push(format!("Shell history:  {}", self.shell_history));

        lines.push(String::new());

        // R-related environment variables
        // Format: "VAR_NAME:       value" with aligned colons
        if !self.env_vars.is_empty() {
            lines.push("## Environment Variables".to_string());
            lines.push(String::new());
            for (label, value) in &self.env_vars {
                lines.push(format!("{label}{value}"));
            }
        }

        append_section_separator(&mut lines);

        // IPC policy. Keep each allowlist target on its own line so a
        // long list remains fully visible in the pager's vertical scroll view.
        append_ipc_info_lines(&mut lines, &self.ipc.policy, self.ipc.server_enabled);

        lines
    }
}

pub(super) fn append_ipc_info_lines(
    lines: &mut Vec<String>,
    ipc_policy: &IpcPolicy,
    server_enabled: bool,
) {
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

/// Styles session info lines: bold headings and cyan keys.
pub(super) struct InfoStyler;

impl LineStyler for InfoStyler {
    type Line = String;

    fn style_line(&self, lines: &[String], index: usize) -> Line<'static> {
        style_info_line(&lines[index])
    }
}

/// Apply styling to an info line, returning a ratatui `Line`.
pub(super) fn style_info_line(line: &str) -> Line<'static> {
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
