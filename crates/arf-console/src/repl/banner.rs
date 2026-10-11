//! Startup banner formatting.

use crate::config::{Config, FormatterBackend, ReprexMode};
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
pub fn format_banner(
    config: &Config,
    r_initialized: bool,
    override_notice: Option<&str>,
    formatter_backend: Option<FormatterBackend>,
) -> String {
    let mut lines = Vec::new();

    lines.push(format!("# arf console v{}", env!("CARGO_PKG_VERSION")));
    lines.push(format!("# Edit mode: {}", config.editor.mode));

    match config.startup.reprex {
        ReprexMode::Off => {}
        ReprexMode::On => lines.push(format!(
            "# Reprex: on | Comment: {:?}",
            config.reprex.comment
        )),
        ReprexMode::Format => {
            let formatter_label = match (config.reprex.formatter, formatter_backend) {
                (crate::config::ReprexFormatter::Auto, Some(backend)) => {
                    format!("auto ({})", backend.command())
                }
                (crate::config::ReprexFormatter::Auto, None) => "auto (unavailable)".to_string(),
                (selector, Some(backend)) => format!("{selector} ({})", backend.command()),
                (selector, None) => format!("{selector} (unavailable)"),
            };
            lines.push(format!(
                "# Reprex: format | Comment: {:?} | Formatter: {}",
                config.reprex.comment, formatter_label
            ));
        }
    }

    if r_initialized {
        let r_version = get_r_version();
        if r_version.is_empty() {
            lines.push("# R is ready.".to_string());
        } else {
            lines.push(format!("# R {} is ready.", r_version));
        }
        if let Some(notice) = override_notice {
            lines.push(notice.to_owned());
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

    fn assert_banner_snapshot(name: &str, banner: String) {
        let version_line = format!("# arf console v{}", env!("CARGO_PKG_VERSION"));
        assert!(
            banner.contains(&version_line),
            "banner should contain the package version"
        );
        let banner = banner.replacen(&version_line, "# arf console v<version>", 1);
        insta::assert_snapshot!(name, banner);
    }

    #[test]
    fn test_banner_default_r_initialized() {
        let config = Config::default();
        let banner = format_banner(&config, true, None, Some(FormatterBackend::Air));
        assert_banner_snapshot("banner_default_r_initialized", banner);
    }

    #[test]
    fn test_banner_default_r_not_initialized() {
        let config = Config::default();
        let banner = format_banner(&config, false, None, Some(FormatterBackend::Air));
        assert_banner_snapshot("banner_default_r_not_initialized", banner);
    }

    #[test]
    fn test_banner_reprex_mode() {
        let mut config = Config::default();
        config.startup.reprex = ReprexMode::On;
        let banner = format_banner(&config, true, None, Some(FormatterBackend::Air));
        assert_banner_snapshot("banner_reprex_mode", banner);
    }

    #[test]
    fn test_banner_reprex_custom_comment() {
        let mut config = Config::default();
        config.startup.reprex = ReprexMode::On;
        config.reprex.comment = "## ".to_string();
        let banner = format_banner(&config, true, None, Some(FormatterBackend::Air));
        assert_banner_snapshot("banner_reprex_custom_comment", banner);
    }

    #[test]
    fn test_banner_reprex_format_mode() {
        let mut config = Config::default();
        config.startup.reprex = ReprexMode::Format;
        let banner = format_banner(&config, true, None, Some(FormatterBackend::Air));
        assert_banner_snapshot("banner_reprex_format_mode", banner);
    }

    #[test]
    fn test_banner_vi_mode() {
        let mut config = Config::default();
        config.editor.mode = crate::config::EditorMode::Vi;
        let banner = format_banner(&config, true, None, Some(FormatterBackend::Air));
        assert_banner_snapshot("banner_vi_mode", banner);
    }

    #[test]
    fn test_banner_includes_package_version() {
        let config = Config::default();
        let banner = format_banner(&config, true, None, Some(FormatterBackend::Air));
        assert!(banner.contains(concat!("# arf console v", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn test_banner_all_lines_start_with_comment() {
        let config = Config::default();
        let banner = format_banner(&config, true, None, Some(FormatterBackend::Air));
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

    #[test]
    fn test_banner_includes_r_source_override_line() {
        let config = Config::default();
        let notice = r#"# R source override: version-file .r-version = "4.4""#;
        let banner = format_banner(&config, true, Some(notice), Some(FormatterBackend::Air));
        assert!(banner.contains(&format!("is ready.\n{notice}\n")));
    }
}
