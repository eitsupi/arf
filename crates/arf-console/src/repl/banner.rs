//! Startup banner formatting.

use crate::config::Config;
use crate::editor::prompt::get_r_version;

/// Format the startup banner.
///
/// All banner lines are prefixed with "# " so they appear as R comments,
/// ensuring copy-pasted output remains valid R code (especially useful for reprex mode).
///
/// The banner content varies based on configuration:
/// - Always shows version and edit mode info
/// - Shows reprex mode info when enabled
/// - Shows R initialization status
pub fn format_banner(config: &Config, r_initialized: bool) -> String {
    let mut lines = Vec::new();

    lines.push(format!("# arf console v{}", env!("CARGO_PKG_VERSION")));
    lines.push(format!("# Edit mode: {}", config.editor.mode));

    if config.reprex.enabled {
        lines.push(format!(
            "# Reprex mode: enabled | Comment: {:?}",
            config.reprex.comment
        ));
    }

    if config.reprex.autoformat {
        lines.push("# Auto-format: enabled (using air)".to_string());
    }

    if r_initialized {
        let r_version = get_r_version();
        if r_version.is_empty() {
            lines.push("# R is ready.".to_string());
        } else {
            lines.push(format!("# R {} is ready.", r_version));
        }
    } else {
        lines.push("# R is not initialized. Commands will not be evaluated.".to_string());
    }

    lines.push("# Type :cmds for meta commands list, Ctrl+D to exit.".to_string());
    lines.push(String::new()); // Empty line at the end

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_banner_default_r_initialized() {
        let config = Config::default();
        let banner = format_banner(&config, true);
        insta::assert_snapshot!("banner_default_r_initialized", banner);
    }

    #[test]
    fn test_banner_default_r_not_initialized() {
        let config = Config::default();
        let banner = format_banner(&config, false);
        insta::assert_snapshot!("banner_default_r_not_initialized", banner);
    }

    #[test]
    fn test_banner_reprex_mode() {
        let mut config = Config::default();
        config.reprex.enabled = true;
        let banner = format_banner(&config, true);
        insta::assert_snapshot!("banner_reprex_mode", banner);
    }

    #[test]
    fn test_banner_reprex_custom_comment() {
        let mut config = Config::default();
        config.reprex.enabled = true;
        config.reprex.comment = "## ".to_string();
        let banner = format_banner(&config, true);
        insta::assert_snapshot!("banner_reprex_custom_comment", banner);
    }

    #[test]
    fn test_banner_vi_mode() {
        let mut config = Config::default();
        config.editor.mode = "vi".to_string();
        let banner = format_banner(&config, true);
        insta::assert_snapshot!("banner_vi_mode", banner);
    }

    #[test]
    fn test_banner_all_lines_start_with_comment() {
        let config = Config::default();
        let banner = format_banner(&config, true);
        for line in banner.lines() {
            if !line.is_empty() {
                assert!(
                    line.starts_with("# "),
                    "Banner line should start with '# ': {:?}",
                    line
                );
            }
        }
    }
}
