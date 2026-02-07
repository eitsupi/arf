//! REPL state management.

use crate::config::{
    HistoryForgetConfig, Indicators, ModeIndicatorPosition, PromptDurationConfig, RSourceStatus,
    SpinnerConfig, StatusColorConfig, StatusConfig, ViColorConfig, ViConfig,
};
use crate::editor::prompt::PromptFormatter;
use crate::external::formatter;
use nu_ansi_term::Color;
use reedline::{HistoryItemId, Reedline};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::prompt::RPrompt;

/// Queue for the sponge-like "forget failed commands" feature.
///
/// This mimics fish shell's sponge plugin behavior:
/// - Every command adds an entry: `Some(id)` for failures, `None` for successes
/// - When queue length exceeds `delay`, the oldest entry is removed
/// - If that entry was a failed command (`Some(id)`), it should be deleted from history
///
/// This allows failed commands to remain accessible for `delay` more commands,
/// giving users a chance to use up-arrow to recall and fix typos.
#[derive(Debug, Default)]
pub struct SpongeQueue {
    /// Queue of command entries. Newer commands at front, older at back.
    queue: VecDeque<Option<HistoryItemId>>,
}

impl SpongeQueue {
    /// Create a new empty sponge queue.
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Record a command execution and return any history ID that should be deleted.
    ///
    /// - `failed`: whether the command failed
    /// - `history_id`: the history ID of the command (if available)
    /// - `delay`: how many commands to wait before deleting failed commands
    ///
    /// Returns `Some(id)` if an old failed command should be deleted from history.
    pub fn record_command(
        &mut self,
        failed: bool,
        history_id: Option<HistoryItemId>,
        delay: usize,
    ) -> Option<HistoryItemId> {
        // Add entry: Some(id) for failure, None for success
        let entry = if failed { history_id } else { None };
        self.queue.push_front(entry);

        // Check if we need to purge the oldest entry
        if self.queue.len() > delay
            && let Some(old_entry) = self.queue.pop_back()
        {
            // Return the ID if it was a failed command
            return old_entry;
        }

        None
    }

    /// Drain all remaining failed command IDs from the queue.
    ///
    /// Used during cleanup (e.g., on exit) to delete all tracked failed commands.
    pub fn drain_failed_ids(&mut self) -> impl Iterator<Item = HistoryItemId> + '_ {
        std::iter::from_fn(move || {
            while let Some(entry) = self.queue.pop_back() {
                if let Some(id) = entry {
                    return Some(id);
                }
            }
            None
        })
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// Convert nu_ansi_term::Color to ANSI escape code string.
fn color_to_ansi_code(color: Color) -> String {
    use nu_ansi_term::Style;
    match color {
        Color::Default => String::new(),
        c => {
            let style = Style::new().fg(c);
            // Get just the prefix (start code) without the suffix
            style.prefix().to_string()
        }
    }
}

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
    /// Queue for the sponge feature (tracks commands to potentially delete).
    pub sponge_queue: SpongeQueue,
}

/// Runtime configuration for prompts that can be modified during the session.
pub struct PromptRuntimeConfig {
    /// Prompt formatter for expanding placeholders.
    prompt_formatter: PromptFormatter,
    /// Main prompt template (unexpanded, e.g., "{status}R {version}> ").
    main_template: String,
    /// Continuation prompt template (unexpanded).
    continuation_template: String,
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
    /// Command duration configuration (format and threshold).
    duration_config: PromptDurationConfig,
    /// Color for command duration indicator.
    duration_color: Color,
    /// When the last command started executing.
    last_command_start: Option<Instant>,
    /// How long the last command took to execute.
    last_command_duration: Option<Duration>,
    /// Spinner configuration for busy indicator.
    spinner_config: SpinnerConfig,
    /// Vi mode configuration (symbols).
    vi_config: ViConfig,
    /// Vi mode colors for prompt indicator.
    vi_colors: ViColorConfig,
}

impl PromptRuntimeConfig {
    /// Create a builder for `PromptRuntimeConfig`.
    ///
    /// Required parameters are the prompt formatter and template strings.
    /// All other fields default to sensible values and can be set via builder methods.
    pub fn builder(
        prompt_formatter: PromptFormatter,
        main_template: impl Into<String>,
        continuation_template: impl Into<String>,
        shell_template: impl Into<String>,
    ) -> PromptRuntimeConfigBuilder {
        PromptRuntimeConfigBuilder::new(
            prompt_formatter,
            main_template,
            continuation_template,
            shell_template,
        )
    }

    pub fn build_main_prompt(&self) -> RPrompt {
        if self.shell_enabled {
            // In shell mode, use shell_template as the main prompt (no mode indicator)
            // Expand placeholders (including {cwd}) dynamically each time
            let shell_format = self.prompt_formatter.format(&self.shell_template);
            let cont_format = self.prompt_formatter.format(&self.continuation_template);
            RPrompt::new(shell_format, cont_format)
                .with_colors(
                    self.shell_color,
                    self.continuation_color,
                    self.mode_indicator_color,
                )
                .with_vi_symbol(self.vi_config.symbol.clone())
                .with_vi_colors(self.vi_colors.clone())
        } else {
            // In R mode, use main_template with optional mode indicator
            // Expand placeholders (including {cwd}) dynamically each time
            let main_format = self.prompt_formatter.format(&self.main_template);
            let cont_format = self.prompt_formatter.format(&self.continuation_template);
            let mode_indicator = self.current_mode_indicator();

            // Determine prompt color based on status mode
            let prompt_color = self.get_status_prompt_color();

            // Expand {status} placeholder, passing prompt_color to restore after symbol
            let main_format = self.expand_status_placeholder(&main_format, prompt_color);

            // Expand {duration} placeholder for command execution time
            let main_format = self.expand_duration_placeholder(&main_format, prompt_color);

            RPrompt::new(main_format, cont_format)
                .with_mode_indicator(mode_indicator, self.mode_indicator_position)
                .with_colors(
                    prompt_color,
                    self.continuation_color,
                    self.mode_indicator_color,
                )
                .with_vi_symbol(self.vi_config.symbol.clone())
                .with_vi_colors(self.vi_colors.clone())
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

            format!("{}{}", status_style.paint(symbol), prompt_style.prefix())
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

    /// Record the start time of a command execution.
    pub fn set_command_start(&mut self) {
        self.last_command_start = Some(Instant::now());
    }

    /// Calculate and store the duration since the last command start.
    ///
    /// Should be called when R returns to the command prompt (alongside `set_last_command_failed`).
    pub fn set_command_duration(&mut self) {
        self.last_command_duration = self.last_command_start.take().map(|start| start.elapsed());
    }

    /// Expand the {duration} placeholder based on duration config and last command duration.
    ///
    /// Shows the duration only when it exceeds the configured threshold.
    /// The format string from config is used to wrap the time value (e.g., "{value} " → "5s ").
    /// The formatted text is colored with the duration color.
    /// After the text, the prompt_color is restored so the rest of the prompt keeps its color.
    fn expand_duration_placeholder(&self, template: &str, prompt_color: Color) -> String {
        use nu_ansi_term::Style;

        if !template.contains("{duration}") {
            return template.to_string();
        }

        let duration_str = match self.last_command_duration {
            Some(duration)
                if duration.as_millis() >= u128::from(self.duration_config.threshold_ms) =>
            {
                let time_str = render_time(duration);
                // Apply format string: replace {value} with the time string
                let formatted = self.duration_config.format.replace("{value}", &time_str);
                let duration_style = match self.duration_color {
                    Color::Default => Style::new(),
                    c => Style::new().fg(c),
                };
                let prompt_style = match prompt_color {
                    Color::Default => Style::new(),
                    c => Style::new().fg(c),
                };
                format!(
                    "{}{}",
                    duration_style.paint(&formatted),
                    prompt_style.prefix()
                )
            }
            _ => String::new(),
        };

        template.replace("{duration}", &duration_str)
    }

    pub fn build_cont_prompt(&self) -> RPrompt {
        let cont_format = self.prompt_formatter.format(&self.continuation_template);
        let mode_indicator = self.current_mode_indicator();
        RPrompt::new(cont_format.clone(), cont_format)
            .with_mode_indicator(mode_indicator, self.mode_indicator_position)
            .with_colors(
                self.continuation_color,
                self.continuation_color,
                self.mode_indicator_color,
            )
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

    /// Start the spinner if enabled and not in shell mode.
    ///
    /// The spinner provides visual feedback that R is evaluating code.
    /// It is automatically stopped when R produces output or the next prompt appears.
    pub fn start_spinner(&self) {
        // Don't show spinner in shell mode (shell commands have their own feedback)
        if self.shell_enabled {
            return;
        }
        // Don't show spinner if frames are empty (disabled)
        if self.spinner_config.frames.is_empty() {
            return;
        }
        arf_libr::start_spinner();
    }
}

/// Format a duration into a human-readable string (starship-style).
///
/// Examples:
/// - 800ms → "800ms"
/// - 5.2s → "5s"
/// - 90s → "1m30s"
/// - 3661s → "1h1m1s"
/// - 86400s → "1d0h0m0s"
///
/// For durations under 1 second, milliseconds are shown (e.g., "800ms").
/// For longer durations, leading zero units are skipped, but once a non-zero unit
/// appears, all subsequent units are included (even if zero).
fn render_time(duration: Duration) -> String {
    let total_secs = duration.as_secs();

    // Show milliseconds for sub-second durations to avoid confusing "0s"
    if total_secs == 0 {
        return format!("{}ms", duration.as_millis());
    }

    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    let mut result = String::new();
    let mut started = false;

    if days > 0 {
        result.push_str(&format!("{}d", days));
        started = true;
    }
    if started || hours > 0 {
        result.push_str(&format!("{}h", hours));
        started = true;
    }
    if started || minutes > 0 {
        result.push_str(&format!("{}m", minutes));
    }
    result.push_str(&format!("{}s", seconds));

    result
}

/// Builder for [`PromptRuntimeConfig`].
///
/// Created via [`PromptRuntimeConfig::builder()`]. Only the prompt formatter
/// and template strings are required; all other fields have sensible defaults.
pub struct PromptRuntimeConfigBuilder {
    prompt_formatter: PromptFormatter,
    main_template: String,
    continuation_template: String,
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
    duration_config: PromptDurationConfig,
    duration_color: Color,
    spinner_config: SpinnerConfig,
    vi_config: ViConfig,
    vi_colors: ViColorConfig,
}

impl PromptRuntimeConfigBuilder {
    fn new(
        prompt_formatter: PromptFormatter,
        main_template: impl Into<String>,
        continuation_template: impl Into<String>,
        shell_template: impl Into<String>,
    ) -> Self {
        Self {
            prompt_formatter,
            main_template: main_template.into(),
            continuation_template: continuation_template.into(),
            shell_template: shell_template.into(),
            mode_indicator_position: ModeIndicatorPosition::default(),
            reprex_enabled: false,
            reprex_comment: "#> ".to_string(),
            indicators: Indicators::default(),
            autoformat_enabled: false,
            main_color: Color::Default,
            continuation_color: Color::Default,
            shell_color: Color::Default,
            mode_indicator_color: Color::Default,
            status_config: StatusConfig::default(),
            status_colors: StatusColorConfig::default(),
            duration_config: PromptDurationConfig::default(),
            duration_color: Color::Default,
            spinner_config: SpinnerConfig::default(),
            vi_config: ViConfig::default(),
            vi_colors: ViColorConfig::default(),
        }
    }

    pub fn mode_indicator_position(mut self, position: ModeIndicatorPosition) -> Self {
        self.mode_indicator_position = position;
        self
    }

    pub fn reprex(mut self, enabled: bool, comment: impl Into<String>) -> Self {
        self.reprex_enabled = enabled;
        self.reprex_comment = comment.into();
        self
    }

    pub fn indicators(mut self, indicators: Indicators) -> Self {
        self.indicators = indicators;
        self
    }

    pub fn autoformat(mut self, enabled: bool) -> Self {
        self.autoformat_enabled = enabled;
        self
    }

    pub fn main_color(mut self, color: Color) -> Self {
        self.main_color = color;
        self
    }

    pub fn continuation_color(mut self, color: Color) -> Self {
        self.continuation_color = color;
        self
    }

    pub fn shell_color(mut self, color: Color) -> Self {
        self.shell_color = color;
        self
    }

    pub fn mode_indicator_color(mut self, color: Color) -> Self {
        self.mode_indicator_color = color;
        self
    }

    pub fn status(mut self, config: StatusConfig, colors: StatusColorConfig) -> Self {
        self.status_config = config;
        self.status_colors = colors;
        self
    }

    pub fn duration(mut self, config: PromptDurationConfig, color: Color) -> Self {
        self.duration_config = config;
        self.duration_color = color;
        self
    }

    pub fn spinner(mut self, config: SpinnerConfig) -> Self {
        self.spinner_config = config;
        self
    }

    pub fn vi(mut self, config: ViConfig, colors: ViColorConfig) -> Self {
        self.vi_config = config;
        self.vi_colors = colors;
        self
    }

    pub fn build(self) -> PromptRuntimeConfig {
        // Initialize spinner in arf-libr
        arf_libr::set_spinner_frames(&self.spinner_config.frames);
        let color_code = color_to_ansi_code(self.spinner_config.color);
        arf_libr::set_spinner_color(&color_code);

        PromptRuntimeConfig {
            prompt_formatter: self.prompt_formatter,
            main_template: self.main_template,
            continuation_template: self.continuation_template,
            shell_template: self.shell_template,
            mode_indicator_position: self.mode_indicator_position,
            reprex_enabled: self.reprex_enabled,
            reprex_comment: self.reprex_comment,
            indicators: self.indicators,
            autoformat_enabled: self.autoformat_enabled,
            shell_enabled: false,
            main_color: self.main_color,
            continuation_color: self.continuation_color,
            shell_color: self.shell_color,
            mode_indicator_color: self.mode_indicator_color,
            status_config: self.status_config,
            status_colors: self.status_colors,
            last_command_failed: false,
            duration_config: self.duration_config,
            duration_color: self.duration_color,
            last_command_start: None,
            last_command_duration: None,
            spinner_config: self.spinner_config,
            vi_config: self.vi_config,
            vi_colors: self.vi_colors,
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
        PromptRuntimeConfig::builder(PromptFormatter::default(), "r> ", "+  ", "[bash] $ ")
            .reprex(reprex, "#> ")
            .indicators(indicators)
            .autoformat(autoformat)
            .build()
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
        let config = PromptRuntimeConfig::builder(
            PromptFormatter::default(),
            "{cwd_short}> ",
            "+  ",
            "[{shell}] $ ",
        )
        .build();

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
        let config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{cwd}> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .build();

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
        // Some systems resolve symlinks differently, so we also accept absolute paths
        #[cfg(unix)]
        let is_absolute_path = rendered2.starts_with("/");
        #[cfg(windows)]
        let is_absolute_path = rendered2.len() >= 3 && rendered2.chars().nth(1) == Some(':');

        assert!(
            rendered2.contains(&temp_dir.to_string_lossy().to_string()) || is_absolute_path,
            "Prompt should contain temp dir path, got: {}",
            rendered2
        );
    }

    #[test]
    fn test_status_indicator_with_error_symbol() {
        let status_config = StatusConfig {
            symbol: StatusSymbol {
                success: "".to_string(),
                error: "✗ ".to_string(),
            },
            override_prompt_color: false,
        };
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{status}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .status(status_config, StatusColorConfig::default())
                .build();

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
        // Both symbols empty - equivalent to old mode=None
        let status_config = StatusConfig {
            symbol: StatusSymbol {
                success: "".to_string(),
                error: "".to_string(),
            },
            override_prompt_color: false,
        };
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{status}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .status(status_config, StatusColorConfig::default())
                .build();

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
        let status_config = StatusConfig {
            symbol: StatusSymbol {
                success: "✓ ".to_string(),
                error: "✗ ".to_string(),
            },
            override_prompt_color: false,
        };
        let mut config = PromptRuntimeConfig::builder(
            PromptFormatter::default(),
            "r> ", // No {status} placeholder
            "+  ",
            "$ ",
        )
        .mode_indicator_position(ModeIndicatorPosition::None)
        .status(status_config, StatusColorConfig::default())
        .build();

        // Prompt stays the same regardless of status
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");

        config.set_last_command_failed(true);
        let prompt = config.build_main_prompt();
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_status_override_prompt_color() {
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
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{status}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .main_color(Color::LightGreen) // Normal main color
                .status(status_config, status_colors)
                .build();

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

    #[test]
    fn test_spinner_not_started_in_shell_mode() {
        let mut config = create_test_config(false, false);
        config.set_shell(true);
        // In shell mode, start_spinner should be a no-op (no panic, etc.)
        config.start_spinner();
        // Verify shell mode is still enabled
        assert!(config.is_shell_enabled());
    }

    #[test]
    fn test_spinner_not_started_with_empty_frames() {
        // Create config with empty spinner frames (disabled by default)
        let config = create_test_config(false, false);
        // Should not panic when spinner is disabled
        config.start_spinner();
    }

    #[test]
    fn test_render_time_seconds_only() {
        assert_eq!(render_time(Duration::from_secs(5)), "5s");
        assert_eq!(render_time(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn test_render_time_minutes_and_seconds() {
        assert_eq!(render_time(Duration::from_secs(60)), "1m0s");
        assert_eq!(render_time(Duration::from_secs(90)), "1m30s");
        assert_eq!(render_time(Duration::from_secs(3599)), "59m59s");
    }

    #[test]
    fn test_render_time_hours() {
        assert_eq!(render_time(Duration::from_secs(3600)), "1h0m0s");
        assert_eq!(render_time(Duration::from_secs(3661)), "1h1m1s");
        assert_eq!(render_time(Duration::from_secs(7200)), "2h0m0s");
    }

    #[test]
    fn test_render_time_days() {
        assert_eq!(render_time(Duration::from_secs(86400)), "1d0h0m0s");
        assert_eq!(render_time(Duration::from_secs(90061)), "1d1h1m1s");
    }

    #[test]
    fn test_render_time_subsecond_shows_milliseconds() {
        // Sub-second durations show milliseconds
        assert_eq!(render_time(Duration::from_millis(0)), "0ms");
        assert_eq!(render_time(Duration::from_millis(500)), "500ms");
        assert_eq!(render_time(Duration::from_millis(800)), "800ms");
        assert_eq!(render_time(Duration::from_millis(999)), "999ms");
        // Once >= 1s, subsecond precision is truncated to whole seconds
        assert_eq!(render_time(Duration::from_millis(2500)), "2s");
    }

    #[test]
    fn test_duration_placeholder_below_threshold() {
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .build();
        // Simulate a fast command (below default 2000ms threshold)
        config.last_command_duration = Some(Duration::from_millis(500));

        let prompt = config.build_main_prompt();
        // Below threshold -> {duration} should be empty
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_duration_placeholder_above_threshold() {
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .build();
        config.last_command_duration = Some(Duration::from_secs(5));

        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        // Above threshold -> should contain "5s" with default format "{value} "
        assert!(
            rendered.contains("5s"),
            "Should contain duration time, got: {}",
            rendered
        );
        assert!(
            rendered.ends_with("r> "),
            "Should end with prompt, got: {}",
            rendered
        );
    }

    #[test]
    fn test_duration_placeholder_no_data() {
        let config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .build();

        let prompt = config.build_main_prompt();
        // No duration data -> {duration} should be empty
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_duration_placeholder_not_present() {
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .build();
        config.last_command_duration = Some(Duration::from_secs(5));

        let prompt = config.build_main_prompt();
        // No {duration} in template -> prompt unchanged
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_duration_custom_threshold() {
        let duration_config = PromptDurationConfig {
            threshold_ms: 500,
            ..PromptDurationConfig::default()
        };
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .duration(duration_config, Color::Default)
                .build();
        // 600ms > 500ms threshold, sub-second shows milliseconds
        config.last_command_duration = Some(Duration::from_millis(600));

        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        assert!(
            rendered.contains("600ms"),
            "Should contain duration time in milliseconds, got: {}",
            rendered
        );
    }

    #[test]
    fn test_duration_custom_format() {
        let duration_config = PromptDurationConfig {
            format: "took {value} ".to_string(),
            threshold_ms: 2000,
        };
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .duration(duration_config, Color::Default)
                .build();
        config.last_command_duration = Some(Duration::from_secs(5));

        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        // Custom format "took {value} " should produce "took 5s "
        assert!(
            rendered.contains("took 5s"),
            "Should contain formatted duration, got: {}",
            rendered
        );
        assert!(
            rendered.ends_with("r> "),
            "Should end with prompt, got: {}",
            rendered
        );
    }

    #[test]
    fn test_duration_format_with_brackets() {
        let duration_config = PromptDurationConfig {
            format: "({value}) ".to_string(),
            threshold_ms: 2000,
        };
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .duration(duration_config, Color::Default)
                .build();
        config.last_command_duration = Some(Duration::from_secs(90));

        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        // Custom format "({value}) " should produce "(1m30s) "
        assert!(
            rendered.contains("(1m30s)"),
            "Should contain bracketed duration, got: {}",
            rendered
        );
    }

    #[test]
    fn test_duration_format_without_value_placeholder() {
        let duration_config = PromptDurationConfig {
            format: "slow! ".to_string(),
            threshold_ms: 2000,
        };
        let mut config =
            PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .duration(duration_config, Color::Default)
                .build();
        config.last_command_duration = Some(Duration::from_secs(5));

        let prompt = config.build_main_prompt();
        let rendered = prompt.render_prompt_left();
        // Format without {value}: only static text "slow! " is shown
        assert!(
            rendered.contains("slow!"),
            "Should contain static text from format, got: {}",
            rendered
        );
        assert!(
            !rendered.contains("5s"),
            "Should not contain time value when {{value}} is absent, got: {}",
            rendered
        );
    }
}
