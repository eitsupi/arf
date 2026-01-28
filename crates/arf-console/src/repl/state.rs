//! REPL state management.

use crate::config::{
    HistoryForgetConfig, Indicators, ModeIndicatorPosition, RSourceStatus, StatusColorConfig,
    StatusConfig,
};
use crate::editor::prompt::PromptFormatter;
use crate::external::formatter;
use nu_ansi_term::Color;
use reedline::{HistoryItemId, Reedline};
use std::collections::VecDeque;
use std::path::PathBuf;

use super::prompt::RPrompt;

/// State shared between the REPL and the ReadConsole callback.
pub struct ReplState {
    /// Line editor for R mode.
    pub line_editor: Reedline,
    /// Line editor for shell mode (with separate history).
    pub shell_line_editor: Reedline,
    pub prompt_config: PromptRuntimeConfig,
    pub should_exit: bool,
    /// Path to the config file (for :info command).
    pub config_path: Option<PathBuf>,
    /// Path to the R history database (for :history commands).
    pub r_history_path: Option<PathBuf>,
    /// Path to the Shell history database (for :history commands).
    pub shell_history_path: Option<PathBuf>,
    /// How R was resolved at startup (for :info display and :switch gating).
    pub r_source_status: RSourceStatus,
    /// Configuration for the sponge-like "forget failed commands" feature.
    pub forget_config: HistoryForgetConfig,
    /// Queue of history item IDs for failed commands (for sponge feature).
    /// Newer failures are at the front, older ones at the back.
    pub failed_commands_queue: VecDeque<HistoryItemId>,
}

/// Runtime configuration for prompts that can be modified during the session.
pub struct PromptRuntimeConfig {
    /// Prompt formatter for expanding placeholders.
    prompt_formatter: PromptFormatter,
    /// Main prompt template (unexpanded, e.g., "{status}R {version}> ").
    main_template: String,
    /// Continuation prompt template (unexpanded).
    cont_template: String,
    /// Shell mode prompt template (unexpanded, e.g., "[{shell}] $ ").
    shell_template: String,
    mode_indicator_position: ModeIndicatorPosition,
    reprex_enabled: bool,
    pub reprex_comment: String,
    indicators: Indicators,
    /// Auto-format R code before execution (using air).
    autoformat_enabled: bool,
    /// Shell mode enabled (input goes to system shell instead of R).
    shell_enabled: bool,
    /// Color for the main R prompt.
    main_color: Color,
    /// Color for the continuation prompt.
    continuation_color: Color,
    /// Color for the shell prompt.
    shell_color: Color,
    /// Color for mode indicators.
    mode_indicator_color: Color,
    /// Command status indicator configuration.
    status_config: StatusConfig,
    /// Colors for command status indicator.
    status_colors: StatusColorConfig,
    /// Whether the last command failed (for status indicator).
    last_command_failed: bool,
}

impl PromptRuntimeConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        prompt_formatter: PromptFormatter,
        main_template: String,
        cont_template: String,
        shell_template: String,
        mode_indicator_position: ModeIndicatorPosition,
        reprex_enabled: bool,
        reprex_comment: String,
        indicators: Indicators,
        autoformat_enabled: bool,
        main_color: Color,
        continuation_color: Color,
        shell_color: Color,
        mode_indicator_color: Color,
        status_config: StatusConfig,
        status_colors: StatusColorConfig,
    ) -> Self {
        Self {
            prompt_formatter,
            main_template,
            cont_template,
            shell_template,
            mode_indicator_position,
            reprex_enabled,
            reprex_comment,
            indicators,
            autoformat_enabled,
            shell_enabled: false,
            main_color,
            continuation_color,
            shell_color,
            mode_indicator_color,
            status_config,
            status_colors,
            last_command_failed: false,
        }
    }

    pub fn build_main_prompt(&self) -> RPrompt {
        if self.shell_enabled {
            // In shell mode, use shell_template as the main prompt (no mode indicator)
            // Expand placeholders (including {cwd}) dynamically each time
            let shell_format = self.prompt_formatter.format(&self.shell_template);
            let cont_format = self.prompt_formatter.format(&self.cont_template);
            RPrompt::new(shell_format, cont_format)
                .with_colors(self.shell_color, self.continuation_color, self.mode_indicator_color)
        } else {
            // In R mode, use main_template with optional mode indicator
            // Expand placeholders (including {cwd}) dynamically each time
            let main_format = self.prompt_formatter.format(&self.main_template);
            let cont_format = self.prompt_formatter.format(&self.cont_template);
            let mode_indicator = self.current_mode_indicator();

            // Determine prompt color based on status mode
            let prompt_color = self.get_status_prompt_color();

            // Expand {status} placeholder, passing prompt_color to restore after symbol
            let main_format = self.expand_status_placeholder(&main_format, prompt_color);

            RPrompt::new(main_format, cont_format)
                .with_mode_indicator(mode_indicator, self.mode_indicator_position)
                .with_colors(prompt_color, self.continuation_color, self.mode_indicator_color)
        }
    }

    /// Expand the {status} placeholder based on status config and last command result.
    ///
    /// The symbol is colored with the status color.
    /// After the symbol, the prompt_color is applied to ensure the rest of the prompt
    /// maintains its color (otherwise the symbol's ANSI reset would clear all colors).
    fn expand_status_placeholder(&self, template: &str, prompt_color: Color) -> String {
        use nu_ansi_term::Style;

        if !template.contains("{status}") {
            return template.to_string();
        }

        let symbol = if self.last_command_failed {
            &self.status_config.symbol.error
        } else {
            &self.status_config.symbol.success
        };

        let colored_symbol = if symbol.is_empty() {
            String::new()
        } else {
            // Color the symbol with status color
            let status_color = if self.last_command_failed {
                self.status_colors.error
            } else {
                self.status_colors.success
            };
            let status_style = match status_color {
                Color::Default => Style::new(),
                c => Style::new().fg(c),
            };

            // After the symbol, apply prompt_color so the rest of the prompt
            // maintains its color (prefix() doesn't add reset at end)
            let prompt_style = match prompt_color {
                Color::Default => Style::new(),
                c => Style::new().fg(c),
            };

            format!(
                "{}{}",
                status_style.paint(symbol),
                prompt_style.prefix()
            )
        };

        template.replace("{status}", &colored_symbol)
    }

    /// Get the prompt color based on status config.
    ///
    /// When `override_prompt_color` is true, returns status-based color.
    /// Otherwise, returns the normal main prompt color.
    fn get_status_prompt_color(&self) -> Color {
        if self.status_config.override_prompt_color {
            if self.last_command_failed {
                self.status_colors.error
            } else {
                self.status_colors.success
            }
        } else {
            self.main_color
        }
    }

    /// Set whether the last command failed.
    pub fn set_last_command_failed(&mut self, failed: bool) {
        self.last_command_failed = failed;
    }

    pub fn build_cont_prompt(&self) -> RPrompt {
        let cont_format = self.prompt_formatter.format(&self.cont_template);
        let mode_indicator = self.current_mode_indicator();
        RPrompt::new(cont_format.clone(), cont_format)
            .with_mode_indicator(mode_indicator, self.mode_indicator_position)
            .with_colors(self.continuation_color, self.continuation_color, self.mode_indicator_color)
    }

    fn current_mode_indicator(&self) -> Option<String> {
        if self.mode_indicator_position == ModeIndicatorPosition::None {
            return None;
        }
        // Note: shell mode uses shell_format directly, so no indicator here
        if self.reprex_enabled && self.autoformat_enabled {
            // Show autoformat indicator when both reprex and autoformat are enabled
            Some(self.indicators.autoformat.clone())
        } else if self.reprex_enabled {
            Some(self.indicators.reprex.clone())
        } else {
            None
        }
    }

    pub fn is_shell_enabled(&self) -> bool {
        self.shell_enabled
    }

    pub fn set_shell(&mut self, enabled: bool) {
        self.shell_enabled = enabled;
    }

    pub fn is_reprex_enabled(&self) -> bool {
        self.reprex_enabled
    }

    pub fn set_reprex(&mut self, enabled: bool, comment: Option<&str>) {
        self.reprex_enabled = enabled;
        if let Some(c) = comment {
            self.reprex_comment = c.to_string();
        }
        arf_libr::set_reprex_mode(self.reprex_enabled, &self.reprex_comment);
    }

    pub fn toggle_reprex(&mut self) {
        self.set_reprex(!self.reprex_enabled, None);
    }

    pub fn is_autoformat_enabled(&self) -> bool {
        self.autoformat_enabled
    }

    pub fn toggle_autoformat(&mut self) {
        self.autoformat_enabled = !self.autoformat_enabled;
    }

    /// Format R code if autoformat is enabled and reprex mode is active.
    ///
    /// Formatting only applies in reprex mode where the formatted code is displayed.
    /// In normal mode, formatting would be invisible to the user, so we skip it
    /// to avoid unnecessary resource usage.
    ///
    /// Returns the formatted code, or the original code if formatting is skipped or fails.
    pub fn maybe_format_code(&self, code: &str) -> String {
        if self.autoformat_enabled && self.reprex_enabled {
            formatter::format_code(code)
        } else {
            code.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StatusSymbol;
    use reedline::Prompt;

    fn create_test_config(reprex: bool, autoformat: bool) -> PromptRuntimeConfig {
        create_test_config_with_indicators(reprex, autoformat, Indicators::default())
    }

    fn create_test_config_with_indicators(
        reprex: bool,
        autoformat: bool,
        indicators: Indicators,
    ) -> PromptRuntimeConfig {
        let formatter = PromptFormatter::default();
        PromptRuntimeConfig::new(
            formatter,
            "r> ".to_string(),
            "+  ".to_string(),
            "[bash] $ ".to_string(),
            ModeIndicatorPosition::Prefix,
            reprex,
            "#> ".to_string(),
            indicators,
            autoformat,
            Color::Default,
            Color::Default,
            Color::Default,
            Color::Default,
            StatusConfig::default(),
            StatusColorConfig::default(),
        )
    }

    #[test]
    fn test_prompt_runtime_config_build_main_prompt() {
        let config = create_test_config(false, false);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_prompt_runtime_config_reprex_mode_indicator() {
        let config = create_test_config(true, false);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");
    }

    #[test]
    fn test_prompt_runtime_config_toggle_reprex() {
        let mut config = create_test_config(false, false);

        assert!(!config.is_reprex_enabled());
        config.toggle_reprex();
        assert!(config.is_reprex_enabled());
        config.toggle_reprex();
        assert!(!config.is_reprex_enabled());
    }

    #[test]
    fn test_prompt_runtime_config_set_reprex_with_comment() {
        let mut config = create_test_config(false, false);

        config.set_reprex(true, Some("## "));
        assert!(config.is_reprex_enabled());
        assert_eq!(config.reprex_comment, "## ");
    }

    #[test]
    fn test_prompt_runtime_config_shell_mode_prompt() {
        let mut config = create_test_config(false, false);

        // Initially R mode prompt
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");

        // Enable shell mode - uses shell_format as prompt
        config.set_shell(true);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[bash] $ ");

        // Shell mode prompt ignores reprex mode
        config.set_reprex(true, None);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[bash] $ ");

        // Disable shell mode, reprex shows
        config.set_shell(false);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");
    }

    #[test]
    fn test_prompt_runtime_config_autoformat_mode_indicator() {
        let mut config = create_test_config(false, false);

        // Initially no indicator
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");

        // Enable reprex mode - shows reprex indicator
        config.set_reprex(true, None);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");

        // Enable autoformat - now shows autoformat indicator instead
        config.toggle_autoformat();
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[format] r> ");

        // Disable reprex - no indicator (autoformat alone doesn't show)
        config.set_reprex(false, None);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");

        // Re-enable reprex - autoformat indicator shows again
        config.set_reprex(true, None);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[format] r> ");

        // Disable autoformat - back to reprex indicator
        config.toggle_autoformat();
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");
    }

    #[test]
    fn test_prompt_runtime_config_custom_autoformat_indicator() {
        let indicators = Indicators {
            autoformat: "[AIR] ".to_string(),
            ..Indicators::default()
        };

        let config = create_test_config_with_indicators(true, true, indicators);

        // Shows custom autoformat indicator
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "[AIR] r> ");
    }

    #[test]
    fn test_prompt_runtime_config_cwd_placeholder_expansion() {
        // Test that {cwd} and {cwd_short} placeholders are expanded dynamically
        let formatter = PromptFormatter::default();
        let config = PromptRuntimeConfig::new(
            formatter,
            "{cwd_short}> ".to_string(), // Template with cwd placeholder
            "+  ".to_string(),
            "[{shell}] $ ".to_string(),
            ModeIndicatorPosition::Prefix,
            false,
            "#> ".to_string(),
            Indicators::default(),
            false,
            Color::Default,
            Color::Default,
            Color::Default,
            Color::Default,
            StatusConfig::default(),
            StatusColorConfig::default(),
        );

        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left().to_string();

        // The cwd_short should be expanded to the current directory's basename
        // It should NOT contain the literal "{cwd_short}" placeholder
        assert!(
            !rendered.contains("{cwd_short}"),
            "Placeholder should be expanded, got: {}",
            rendered
        );
        assert!(
            rendered.ends_with("> "),
            "Prompt should end with '> ', got: {}",
            rendered
        );
    }

    #[test]
    fn test_prompt_runtime_config_dynamic_cwd_update() {
        // Test that build_main_prompt() returns updated cwd after directory change
        let formatter = PromptFormatter::default();
        let config = PromptRuntimeConfig::new(
            formatter,
            "{cwd}> ".to_string(), // Template with full cwd placeholder
            "+  ".to_string(),
            "$ ".to_string(),
            ModeIndicatorPosition::None,
            false,
            "#> ".to_string(),
            Indicators::default(),
            false,
            Color::Default,
            Color::Default,
            Color::Default,
            Color::Default,
            StatusConfig::default(),
            StatusColorConfig::default(),
        );

        // Get the current directory
        let original_cwd = std::env::current_dir().unwrap();
        let prompt1 = config.build_main_prompt();
        let rendered1 = prompt1.render_prompt_left().to_string();

        // Change to a temporary directory
        let temp_dir = std::env::temp_dir();
        std::env::set_current_dir(&temp_dir).unwrap();

        // Build prompt again - should reflect the new directory
        let prompt2 = config.build_main_prompt();
        let rendered2 = prompt2.render_prompt_left().to_string();

        // Restore original directory
        std::env::set_current_dir(&original_cwd).unwrap();

        // The two prompts should be different if cwd changed
        // (unless original_cwd == temp_dir, which is unlikely)
        if original_cwd != temp_dir {
            assert_ne!(
                rendered1, rendered2,
                "Prompt should update when cwd changes.\nBefore: {}\nAfter: {}",
                rendered1, rendered2
            );
        }

        // Verify the prompt contains the temp directory path
        assert!(
            rendered2.contains(&temp_dir.to_string_lossy().to_string())
                || rendered2.starts_with("/"), // Some systems resolve symlinks differently
            "Prompt should contain temp dir path, got: {}",
            rendered2
        );
    }

    #[test]
    fn test_status_indicator_with_error_symbol() {
        let formatter = PromptFormatter::default();
        let status_config = StatusConfig {
            symbol: StatusSymbol {
                success: "".to_string(),
                error: "✗ ".to_string(),
            },
            override_prompt_color: false,
        };
        let mut config = PromptRuntimeConfig::new(
            formatter,
            "{status}r> ".to_string(),
            "+  ".to_string(),
            "$ ".to_string(),
            ModeIndicatorPosition::None,
            false,
            "#> ".to_string(),
            Indicators::default(),
            false,
            Color::Default,
            Color::Default,
            Color::Default,
            Color::Default,
            status_config,
            StatusColorConfig::default(),
        );

        // Initially no error - empty status symbol
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");

        // After command failure - shows error symbol (with color)
        config.set_last_command_failed(true);
        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        // Symbol should contain "✗ " (possibly with ANSI color codes)
        assert!(
            rendered.contains("✗"),
            "Should contain error symbol, got: {}",
            rendered
        );
        assert!(
            rendered.ends_with("r> "),
            "Should end with prompt, got: {}",
            rendered
        );

        // After successful command - back to empty
        config.set_last_command_failed(false);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_status_indicator_with_empty_symbols() {
        let formatter = PromptFormatter::default();
        // Both symbols empty - equivalent to old mode=None
        let status_config = StatusConfig {
            symbol: StatusSymbol {
                success: "".to_string(),
                error: "".to_string(),
            },
            override_prompt_color: false,
        };
        let mut config = PromptRuntimeConfig::new(
            formatter,
            "{status}r> ".to_string(),
            "+  ".to_string(),
            "$ ".to_string(),
            ModeIndicatorPosition::None,
            false,
            "#> ".to_string(),
            Indicators::default(),
            false,
            Color::Default,
            Color::Default,
            Color::Default,
            Color::Default,
            status_config,
            StatusColorConfig::default(),
        );

        // With empty symbols, status placeholder should expand to empty string
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");

        // Even after failure, still empty
        config.set_last_command_failed(true);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_status_without_placeholder() {
        // Test that status config has no effect when {status} placeholder is absent
        let formatter = PromptFormatter::default();
        let status_config = StatusConfig {
            symbol: StatusSymbol {
                success: "✓ ".to_string(),
                error: "✗ ".to_string(),
            },
            override_prompt_color: false,
        };
        let mut config = PromptRuntimeConfig::new(
            formatter,
            "r> ".to_string(), // No {status} placeholder
            "+  ".to_string(),
            "$ ".to_string(),
            ModeIndicatorPosition::None,
            false,
            "#> ".to_string(),
            Indicators::default(),
            false,
            Color::Default,
            Color::Default,
            Color::Default,
            Color::Default,
            status_config,
            StatusColorConfig::default(),
        );

        // Prompt stays the same regardless of status
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");

        config.set_last_command_failed(true);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_status_override_prompt_color() {
        let formatter = PromptFormatter::default();
        let status_config = StatusConfig {
            symbol: StatusSymbol {
                success: "".to_string(),
                error: "✗ ".to_string(),
            },
            override_prompt_color: true, // Enable prompt color override
        };
        let status_colors = StatusColorConfig {
            success: Color::Green,
            error: Color::Red,
        };
        let mut config = PromptRuntimeConfig::new(
            formatter,
            "{status}r> ".to_string(),
            "+  ".to_string(),
            "$ ".to_string(),
            ModeIndicatorPosition::None,
            false,
            "#> ".to_string(),
            Indicators::default(),
            false,
            Color::LightGreen, // Normal main color
            Color::Default,
            Color::Default,
            Color::Default,
            status_config,
            status_colors,
        );

        // On success, prompt should use success color (Green)
        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        // The prompt text "r> " should be colored with Green
        assert!(
            rendered.contains("r> "),
            "Should contain prompt text, got: {}",
            rendered
        );

        // On failure, prompt should use error color (Red)
        config.set_last_command_failed(true);
        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        // Should contain both the error symbol and prompt
        assert!(
            rendered.contains("✗") && rendered.contains("r>"),
            "Should contain error symbol and prompt, got: {}",
            rendered
        );
    }
}
