//! REPL (Read-Eval-Print Loop) implementation.

mod banner;
mod meta_command;
mod prompt;
mod reprex;
mod shell;
pub(crate) mod state;

use crate::completion::completer::{CombinedCompleter, MetaCommandCompleter};
use crate::completion::menu::{FunctionAwareMenu, StateSyncHistoryMenu};
use crate::config::{
    AutoSuggestions, Config, EditorMode, ModeIndicatorPosition, RSourceStatus, history_dir,
};
use crate::editor::hinter::RLanguageHinter;
use crate::editor::mode::new_editor_state_ref;
use crate::editor::prompt::PromptFormatter;
use crate::highlighter::{CombinedHighlighter, MetaCommandHighlighter};
use crate::history::FuzzyHistory;
use anyhow::Result;
use crossterm::{
    ExecutableCommand,
    style::Stylize,
    terminal::{self, ClearType},
};
use nu_ansi_term::{Color, Style};
use reedline::{
    DefaultHinter, Emacs, IdeMenu, ListMenu, MenuBuilder, Reedline, ReedlineMenu, Signal,
    SqliteBackedHistory, Vi, default_emacs_keybindings, default_vi_insert_keybindings,
    default_vi_normal_keybindings,
};
use std::cell::RefCell;
use std::io;

use crate::editor::keybindings::{
    add_auto_match_keybindings, add_common_keybindings, add_key_map_keybindings,
    wrap_edit_mode_with_conditional_rules,
};
use crate::editor::validator::RValidator;
use banner::format_banner;
use meta_command::{MetaCommandResult, process_meta_command};
use prompt::RPrompt;
use reprex::{clear_input_lines, strip_reprex_output};
use shell::{execute_shell_command, restart_process};
use state::{PromptRuntimeConfig, ReplState};

// Thread-local storage for the REPL state.
// This allows the ReadConsole callback to access the line editor.
thread_local! {
    static REPL_STATE: RefCell<Option<ReplState>> = const { RefCell::new(None) };
}

/// Prefix for arf messages to distinguish them from R output.
/// Uses R comment syntax so messages don't interfere with R code.
pub(crate) const ARF_PREFIX: &str = "# [arf]";

/// Print an arf message to stdout.
macro_rules! arf_println {
    ($($arg:tt)*) => {
        println!("{} {}", $crate::repl::ARF_PREFIX, format_args!($($arg)*))
    };
}

/// Print an arf message to stderr.
macro_rules! arf_eprintln {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::repl::ARF_PREFIX, format_args!($($arg)*))
    };
}

pub(crate) use arf_eprintln;
pub(crate) use arf_println;

/// The main REPL structure.
pub struct Repl {
    config: Config,
    /// Path to the config file (if specified via --config, or the default XDG path).
    config_path: Option<std::path::PathBuf>,
    /// Whether the config file was loaded successfully (false if parse error occurred).
    config_load_ok: bool,
    /// How R was resolved at startup (determines if :switch is available).
    r_source_status: RSourceStatus,
    r_initialized: bool,
    prompt_formatter: PromptFormatter,
}

impl Repl {
    /// Create a new REPL with the given configuration.
    ///
    /// The `config_path` should be the path to the config file that was used,
    /// or `None` if using defaults (no config file found).
    ///
    /// The `r_source_status` describes how R was resolved at startup,
    /// which determines if features like `:switch` are available.
    pub fn new(
        config: Config,
        config_path: Option<std::path::PathBuf>,
        config_load_ok: bool,
        r_source_status: RSourceStatus,
    ) -> Result<Self> {
        // Check if R is initialized
        let r_initialized = arf_libr::r_library().is_ok();

        // Create prompt formatter (caches R version)
        let prompt_formatter = PromptFormatter::new();

        // Set up reprex mode if enabled
        if config.startup.mode.reprex {
            arf_libr::set_reprex_mode(true, &config.mode.reprex.comment);
        }

        Ok(Repl {
            config,
            config_path,
            config_load_ok,
            r_source_status,
            r_initialized,
            prompt_formatter,
        })
    }

    /// Get the R history database path based on configuration.
    fn r_history_path(&self) -> Option<std::path::PathBuf> {
        if self.config.history.disabled {
            return None;
        }
        let dir = self.config.history.dir.clone().or_else(history_dir);
        dir.map(|d| d.join("r.db"))
    }

    /// Get the Shell history database path based on configuration.
    fn shell_history_path(&self) -> Option<std::path::PathBuf> {
        if self.config.history.disabled {
            return None;
        }
        let dir = self.config.history.dir.clone().or_else(history_dir);
        dir.map(|d| d.join("shell.db"))
    }

    /// Create an R language hinter based on config settings.
    ///
    /// Returns `Some(hinter)` if auto_suggestions is enabled, `None` otherwise.
    fn create_r_hinter(&self) -> Option<Box<RLanguageHinter>> {
        match self.config.editor.auto_suggestions {
            AutoSuggestions::None => None,
            AutoSuggestions::All => Some(Box::new(
                RLanguageHinter::new().with_style(Style::new().italic().fg(Color::DarkGray)),
            )),
            AutoSuggestions::Cwd => Some(Box::new(
                RLanguageHinter::new()
                    .with_style(Style::new().italic().fg(Color::DarkGray))
                    .with_cwd_aware(true),
            )),
        }
    }

    /// Run the REPL main loop.
    pub fn run(&mut self) -> Result<()> {
        // Show startup banner unless disabled
        if self.config.startup.show_banner {
            let banner = format_banner(&self.config, self.r_initialized);
            // Apply color to the "not initialized" warning if present
            if !self.r_initialized {
                for line in banner.lines() {
                    if line.contains("R is not initialized") {
                        println!(
                            "# {}",
                            "R is not initialized. Commands will not be evaluated.".yellow()
                        );
                    } else {
                        println!("{}", line);
                    }
                }
            } else {
                print!("{}", banner);
            }
        }

        if self.r_initialized {
            // Use R's main loop with ReadConsole callback
            self.run_with_r_mainloop()?;
        } else {
            // Fall back to standalone mode without R
            self.run_standalone()?;
        }

        Ok(())
    }

    /// Run with R's main loop (run_Rmainloop).
    fn run_with_r_mainloop(&self) -> Result<()> {
        // Create line editor with bracketed paste enabled
        // This allows detecting paste operations and prevents auto-match from
        // interfering with pasted text (e.g., pasting "()" won't become "())")
        let line_editor = Reedline::create().use_bracketed_paste(true);

        // Set up SQLite-backed history for R mode
        let mut line_editor = setup_history(line_editor, self.r_history_path());

        // Set up edit mode (Vi or Emacs) with conditional ':' keybinding
        let editor_state = new_editor_state_ref();
        line_editor = match self.config.editor.mode {
            EditorMode::Vi => {
                let mut insert_keybindings = default_vi_insert_keybindings();
                add_common_keybindings(&mut insert_keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut insert_keybindings);
                }
                add_key_map_keybindings(&mut insert_keybindings, &self.config.editor.key_map);
                let vi = Vi::new(insert_keybindings, default_vi_normal_keybindings());
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    vi,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                ))
            }
            EditorMode::Emacs => {
                let mut keybindings = default_emacs_keybindings();
                add_common_keybindings(&mut keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut keybindings);
                }
                add_key_map_keybindings(&mut keybindings, &self.config.editor.key_map);
                let emacs = Emacs::new(keybindings);
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    emacs,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                ))
            }
        };

        // Set up combined completer (R + meta commands) if completion is enabled
        // When rig is not enabled, :switch is excluded from completion
        if self.config.completion.enabled {
            let completer = Box::new(CombinedCompleter::with_settings_and_rig(
                self.config.completion.timeout_ms,
                self.config.completion.debounce_ms,
                self.config.completion.auto_paren_limit,
                self.r_source_status.rig_enabled(),
            ));
            line_editor = line_editor.with_completer(completer);

            // Set up completion menu with height limit for better UX
            // Use FunctionAwareMenu to handle cursor positioning for function completions
            // Pass editor_state to synchronize shadow tracking after completion
            let ide_menu = IdeMenu::default()
                .with_name("completion_menu")
                .with_max_completion_height(self.config.completion.max_height);
            let completion_menu =
                Box::new(FunctionAwareMenu::new(ide_menu).with_editor_state(editor_state.clone()));
            line_editor = line_editor.with_menu(ReedlineMenu::EngineCompleter(completion_menu));
        }

        // Set up history menu for Ctrl+R search (shows multiple candidates)
        // Use only_buffer_difference(false) so selecting replaces buffer instead of appending
        // See: https://github.com/nushell/nushell/issues/7746
        // Dynamic page size based on terminal height (leave space for prompt and input)
        // Capped by config max_height to avoid overwhelming display on tall terminals
        //
        // TODO: reedline's ListMenu.page_size only limits the first page; subsequent pages
        // use full terminal height. This is a bug in reedline's printable_entries() method.
        // See IdeMenu fix in reedline#781 for reference. Once fixed upstream, this will work
        // correctly for all pages.
        let (_, rows) = terminal::size().unwrap_or((80, 24));
        let terminal_based_size = rows.saturating_sub(5) as usize;
        let config_max_height = self.config.history.menu_max_height as usize;
        let history_page_size = terminal_based_size.min(config_max_height).max(3);
        let list_menu = ListMenu::default()
            .with_name("history_menu")
            .with_only_buffer_difference(false)
            .with_page_size(history_page_size);
        let history_menu =
            Box::new(StateSyncHistoryMenu::new(list_menu).with_editor_state(editor_state.clone()));
        line_editor = line_editor.with_menu(ReedlineMenu::HistoryMenu(history_menu));

        // Set up validator for multiline input
        // Pass editor_state so validator can synchronize shadow state with actual buffer
        line_editor = line_editor.with_validator(Box::new(
            RValidator::new().with_editor_state(editor_state.clone()),
        ));

        // Set up syntax highlighter (R code + meta commands)
        // Pass editor_state so highlighter can sync shadow state on every redraw
        let highlighter = CombinedHighlighter::new(self.config.colors.clone())
            .with_editor_state(editor_state.clone());
        line_editor = line_editor.with_highlighter(Box::new(highlighter));

        // Set up history-based autosuggestion (fish/nushell style)
        // Uses RLanguageHinter for proper R token handling (e.g., |> as single token)
        if let Some(hinter) = self.create_r_hinter() {
            line_editor = line_editor.with_hinter(hinter);
        }

        // Set up idle callback to process R events during input waiting.
        // This allows graphics windows (plot(), help browser) to remain responsive
        // while the user is typing or the editor is waiting for input.
        line_editor = line_editor
            .with_poll_interval(std::time::Duration::from_millis(33))
            .with_idle_callback(Box::new(|| {
                arf_libr::process_r_events();
            }));

        // Create shell line editor with separate history
        let shell_line_editor = self.create_shell_line_editor();

        // Create prompt runtime config with unexpanded templates
        // Templates are expanded dynamically in build_main_prompt() to track cwd changes
        let prompt_config = PromptRuntimeConfig::builder(
            self.prompt_formatter.clone(),
            self.config.prompt.format.clone(),
            self.config.prompt.continuation.clone(),
            self.config.prompt.shell_format.clone(),
        )
        .mode_indicator_position(self.config.prompt.mode_indicator)
        .reprex(
            self.config.startup.mode.reprex,
            self.config.mode.reprex.comment.clone(),
        )
        .indicators(self.config.prompt.indicators.clone())
        .autoformat(self.config.startup.mode.autoformat)
        .main_color(self.config.colors.prompt.main)
        .continuation_color(self.config.colors.prompt.continuation)
        .shell_color(self.config.colors.prompt.shell)
        .mode_indicator_color(self.config.colors.prompt.indicator)
        .status(
            self.config.prompt.status.clone(),
            self.config.colors.prompt.status.clone(),
        )
        .duration(
            self.config.experimental.prompt_duration.clone(),
            self.config.colors.prompt.duration,
        )
        .spinner(self.config.experimental.prompt_spinner.clone())
        .vi(
            self.config.prompt.vi.clone(),
            self.config.colors.prompt.vi.clone(),
        )
        .build();

        // Get history paths for :history commands
        let r_history_path = self.r_history_path();
        let shell_history_path = self.shell_history_path();

        // Store state in thread-local
        REPL_STATE.with(|state| {
            *state.borrow_mut() = Some(ReplState {
                line_editor,
                shell_line_editor,
                prompt_config,
                should_exit: false,
                config_path: self.config_path.clone(),
                config_load_ok: self.config_load_ok,
                r_history_path,
                shell_history_path,
                r_source_status: self.r_source_status.clone(),
                forget_config: self.config.experimental.history_forget.clone(),
                sponge_queue: state::SpongeQueue::new(),
                dir_stack: Vec::new(),
            });
        });

        // Initialize global error handler for rlang/dplyr error detection
        // This sets up globalCallingHandlers() to track error conditions
        // that output to stdout instead of stderr
        let error_handler_code = arf_libr::global_error_handler_code();
        match arf_harp::eval_string_with_visibility(error_handler_code) {
            Ok(_) => {
                log::info!("Global error handler initialized");
                arf_libr::mark_global_error_handler_initialized();
            }
            Err(e) => {
                log::warn!("Failed to initialize global error handler: {:?}", e);
            }
        }

        // Initialize askpass handler (Unix only) to bypass reedline for password input.
        #[cfg(unix)]
        {
            let askpass_handler_code = arf_libr::askpass_handler_code();
            match arf_harp::eval_string_with_visibility(askpass_handler_code) {
                Ok(_) => {
                    log::info!("Askpass handler initialized");
                }
                Err(e) => {
                    log::warn!("Failed to initialize askpass handler: {:?}", e);
                }
            }
        }

        // Set up the ReadConsole callback
        arf_libr::set_read_console_callback(read_console_callback);

        // Run R's main loop - this doesn't return until EOF
        unsafe {
            arf_libr::run_r_mainloop();
        }

        // Sponge cleanup on exit: purge all remaining failed commands in the queue.
        // Note: R's q() may terminate the process before this cleanup completes,
        // so the most recent failed command might remain in history.
        // The main value of sponge is purging OLD failed commands during the session.
        REPL_STATE.with(|state| {
            if let Some(ref mut repl_state) = *state.borrow_mut()
                && repl_state.forget_config.enabled
                && !repl_state.sponge_queue.is_empty()
            {
                for id_to_delete in repl_state.sponge_queue.drain_failed_ids() {
                    let _ = repl_state.line_editor.history_mut().delete(id_to_delete);
                }
                let _ = repl_state.line_editor.sync_history();
            }
        });

        REPL_STATE.with(|state| {
            *state.borrow_mut() = None;
        });

        println!("\nGoodbye!");
        Ok(())
    }

    /// Run without R (standalone mode).
    fn run_standalone(&self) -> Result<()> {
        // Create line editor with bracketed paste enabled
        let line_editor = Reedline::create().use_bracketed_paste(true);

        // Set up SQLite-backed history for R mode
        let mut line_editor = setup_history(line_editor, self.r_history_path());

        // Set up edit mode with conditional ':' keybinding
        let editor_state = new_editor_state_ref();
        line_editor = match self.config.editor.mode {
            EditorMode::Vi => {
                let mut insert_keybindings = default_vi_insert_keybindings();
                add_common_keybindings(&mut insert_keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut insert_keybindings);
                }
                add_key_map_keybindings(&mut insert_keybindings, &self.config.editor.key_map);
                let vi = Vi::new(insert_keybindings, default_vi_normal_keybindings());
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    vi,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                ))
            }
            EditorMode::Emacs => {
                let mut keybindings = default_emacs_keybindings();
                add_common_keybindings(&mut keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut keybindings);
                }
                add_key_map_keybindings(&mut keybindings, &self.config.editor.key_map);
                let emacs = Emacs::new(keybindings);
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    emacs,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                ))
            }
        };

        // Set up history-based autosuggestion (fish/nushell style)
        // Uses RLanguageHinter for proper R token handling (e.g., |> as single token)
        if let Some(hinter) = self.create_r_hinter() {
            line_editor = line_editor.with_hinter(hinter);
        }

        // Mode indicator for special modes (reprex, etc.)
        let mode_position = self.config.prompt.mode_indicator;
        let mode_indicator =
            if self.config.startup.mode.reprex && mode_position != ModeIndicatorPosition::None {
                Some(self.config.prompt.indicators.reprex.clone())
            } else {
                None
            };

        let prompt = RPrompt::new(
            self.prompt_formatter.format(&self.config.prompt.format),
            self.prompt_formatter
                .format(&self.config.prompt.continuation),
        )
        .with_mode_indicator(mode_indicator, mode_position)
        .with_colors(
            self.config.colors.prompt.main,
            self.config.colors.prompt.continuation,
            self.config.colors.prompt.indicator,
        );

        // Minimal prompt config for meta commands (R not available)
        let mut prompt_config =
            PromptRuntimeConfig::builder(self.prompt_formatter.clone(), "R > ", "+   ", "$ ")
                .mode_indicator_position(ModeIndicatorPosition::None)
                .main_color(self.config.colors.prompt.main)
                .continuation_color(self.config.colors.prompt.continuation)
                .shell_color(self.config.colors.prompt.shell)
                .mode_indicator_color(self.config.colors.prompt.indicator)
                .status(
                    self.config.prompt.status.clone(),
                    self.config.colors.prompt.status.clone(),
                )
                .duration(
                    self.config.experimental.prompt_duration.clone(),
                    self.config.colors.prompt.duration,
                )
                .spinner(self.config.experimental.prompt_spinner.clone())
                .vi(
                    self.config.prompt.vi.clone(),
                    self.config.colors.prompt.vi.clone(),
                )
                .build();
        let r_history_path = self.r_history_path();
        let shell_history_path = self.shell_history_path();
        // Separate dir_stack for standalone mode (R not initialized).
        // The R mainloop path stores its own dir_stack in ReplState.
        // These two paths are mutually exclusive, so no sharing is needed.
        let mut dir_stack: Vec<std::path::PathBuf> = Vec::new();

        loop {
            match line_editor.read_line(&prompt) {
                Ok(Signal::Success(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Process meta commands even when R is not initialized
                    // This allows :switch, :quit, :shell, etc. to work
                    if let Some(result) = process_meta_command(
                        &line,
                        &mut prompt_config,
                        &self.config_path,
                        self.config_load_ok,
                        &r_history_path,
                        &shell_history_path,
                        &self.r_source_status,
                        &mut dir_stack,
                    ) {
                        // Clear duration so the previous R command's time
                        // does not persist in the prompt after a meta command.
                        prompt_config.clear_command_duration();
                        match result {
                            MetaCommandResult::Handled => {
                                continue;
                            }
                            MetaCommandResult::Exit => {
                                println!("\nGoodbye!");
                                return Ok(());
                            }
                            MetaCommandResult::Unknown(cmd) => {
                                arf_println!(
                                    "Unknown command: {}. Type :commands for available commands.",
                                    cmd
                                );
                                continue;
                            }
                            MetaCommandResult::ShellExecuted => {
                                continue;
                            }
                            MetaCommandResult::Restart(version) => {
                                restart_process(version.as_deref());
                                continue;
                            }
                        }
                    }

                    // Not a meta command - show R not initialized message
                    println!("{}", format!("[R not initialized] {}", line).dark_grey());
                }
                Ok(Signal::CtrlC) => {
                    // Clear any visible completion menu before printing ^C
                    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
                    println!("^C");
                    continue;
                }
                Ok(Signal::CtrlD) => {
                    // Clear any visible menu before printing farewell message
                    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
                    println!("\nGoodbye!");
                    break;
                }
                Err(err) => {
                    eprintln!("Error: {}", err);
                    break;
                }
            }
        }

        Ok(())
    }

    /// Create a shell mode line editor with separate history.
    ///
    /// Shell mode uses a separate SQLite history database from R mode.
    fn create_shell_line_editor(&self) -> Reedline {
        // Create shell editor with bracketed paste enabled
        let shell_editor = Reedline::create().use_bracketed_paste(true);

        // Set up SQLite-backed history for Shell mode (separate from R)
        let mut shell_editor = setup_history(shell_editor, self.shell_history_path());

        // Use same edit mode as R editor
        shell_editor = match self.config.editor.mode {
            EditorMode::Vi => {
                let mut insert_keybindings = default_vi_insert_keybindings();
                add_common_keybindings(&mut insert_keybindings);
                add_key_map_keybindings(&mut insert_keybindings, &self.config.editor.key_map);
                shell_editor.with_edit_mode(Box::new(Vi::new(
                    insert_keybindings,
                    default_vi_normal_keybindings(),
                )))
            }
            EditorMode::Emacs => {
                let mut keybindings = default_emacs_keybindings();
                add_common_keybindings(&mut keybindings);
                add_key_map_keybindings(&mut keybindings, &self.config.editor.key_map);
                shell_editor.with_edit_mode(Box::new(Emacs::new(keybindings)))
            }
        };

        // Set up meta command completer only (no R completion in shell mode) if completion is enabled
        if self.config.completion.enabled {
            // Exclude Shell mode-irrelevant commands:
            // - `:shell` - already in Shell mode
            // - `:system` - can run system commands directly
            // - `:autoformat`, `:format` - R code formatting
            // - `:restart` - R session restart
            // - `:reprex` - R reproducible examples
            // - `:switch` - R version switching
            // - `:h`, `:help` - R help browser
            let completer = Box::new(MetaCommandCompleter::with_exclusions(vec![
                "shell",
                "system",
                "autoformat",
                "format",
                "restart",
                "reprex",
                "switch",
                "h",
                "help",
            ]));
            shell_editor = shell_editor.with_completer(completer);

            // Set up completion menu with height limit for better UX
            let completion_menu = Box::new(
                IdeMenu::default()
                    .with_name("completion_menu")
                    .with_max_completion_height(self.config.completion.max_height),
            );
            shell_editor = shell_editor.with_menu(ReedlineMenu::EngineCompleter(completion_menu));
        }

        // History menu for shell mode (same setup as main R mode).
        // See reedline#781 TODO note above for page size limitation.
        let (_, rows) = terminal::size().unwrap_or((80, 24));
        let terminal_based_size = rows.saturating_sub(5) as usize;
        let config_max_height = self.config.history.menu_max_height as usize;
        let history_page_size = terminal_based_size.min(config_max_height).max(3);
        let history_menu = Box::new(
            ListMenu::default()
                .with_name("history_menu")
                .with_only_buffer_difference(false)
                .with_page_size(history_page_size),
        );
        shell_editor = shell_editor.with_menu(ReedlineMenu::HistoryMenu(history_menu));

        // Set up highlighter for meta command visual feedback
        shell_editor = shell_editor.with_highlighter(Box::new(MetaCommandHighlighter::new(
            self.config.colors.meta.clone(),
        )));

        // Set up history-based autosuggestion (uses shell history)
        // Note: Shell mode doesn't support cwd filtering; treat All and Cwd the same
        if !matches!(self.config.editor.auto_suggestions, AutoSuggestions::None) {
            let hinter =
                DefaultHinter::default().with_style(Style::new().italic().fg(Color::DarkGray));
            shell_editor = shell_editor.with_hinter(Box::new(hinter));
        }

        // Set up idle callback to process R events during input waiting.
        // Even in shell mode, R graphics windows may be open and need event processing.
        shell_editor = shell_editor
            .with_poll_interval(std::time::Duration::from_millis(33))
            .with_idle_callback(Box::new(|| {
                arf_libr::process_r_events();
            }));

        shell_editor
    }
}

/// ReadConsole callback function.
/// This is called by R when it needs user input.
///
/// With the Validator in place, reedline handles multiline input internally.
/// The callback receives complete expressions (possibly with embedded newlines)
/// from reedline and passes them to R.
fn read_console_callback(r_prompt: &str) -> Option<String> {
    REPL_STATE.with(|state| {
        // Use try_borrow_mut to detect re-entrant calls.
        // This is a defensive measure in case R unexpectedly calls ReadConsole
        // while we're still processing a previous call. This was originally
        // needed when RValidator called harp::is_expression_complete (which
        // invokes R's parser), but is now less critical since we switched to
        // a tree-sitter-r based validator that doesn't call into R.
        let mut guard = match state.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => {
                // Re-entrant call detected - RefCell already borrowed.
                // Return None (EOF) to terminate the nested call.
                // This prevents panic from double borrow.
                return None;
            }
        };
        let state = guard.as_mut()?;

        if state.should_exit {
            return None;
        }

        // Update exit_status for the previous command when a new prompt is shown.
        // This is called when R has finished evaluating and wants new input.
        // Continuation prompts (starting with '+') mean we're still in the same expression.
        // Non-command prompts (menus, etc.) should also not trigger exit status updates.
        if is_r_command_prompt(r_prompt) && !state.prompt_config.is_shell_enabled() {
            let had_error = if state.line_editor.has_last_command_context() {
                let had_error = arf_libr::command_had_error();
                let exit_status = if had_error { 1i64 } else { 0i64 };

                // Use Cell to capture the history item ID through the immutable closure
                let captured_id: std::cell::Cell<Option<reedline::HistoryItemId>> =
                    std::cell::Cell::new(None);
                let _ = state.line_editor.update_last_command_context(&|mut item| {
                    item.exit_status = Some(exit_status);
                    captured_id.set(item.id);
                    item
                });

                // Sponge feature: track all commands and purge old failed ones.
                // See SpongeQueue for the algorithm details.
                if state.forget_config.enabled {
                    // Determine effective delay: use configured delay normally,
                    // or usize::MAX for on_exit_only (defer all deletions until exit)
                    let effective_delay = if state.forget_config.on_exit_only {
                        usize::MAX
                    } else {
                        state.forget_config.delay
                    };

                    if let Some(id_to_delete) = state.sponge_queue.record_command(
                        had_error,
                        captured_id.get(),
                        effective_delay,
                    ) {
                        let _ = state.line_editor.history_mut().delete(id_to_delete);
                    }
                }
                had_error
            } else {
                false
            };

            // Update prompt status indicator for the next prompt
            state.prompt_config.set_last_command_failed(had_error);

            // Calculate duration for the {duration} prompt placeholder
            state.prompt_config.set_command_duration();

            // Reset error state for the next command
            arf_libr::reset_command_error_state();
        }

        loop {
            // Build prompt dynamically from config.
            // We detect the type of prompt R is asking for:
            // - Continuation prompts start with '+' (multiline input)
            // - Command prompts typically end with "> " (R's default prompt)
            // - Non-standard prompts (menus, etc.) are passed through directly
            let prompt = if r_prompt.starts_with('+') {
                state.prompt_config.build_cont_prompt()
            } else if is_r_command_prompt(r_prompt) {
                state.prompt_config.build_main_prompt()
            } else {
                // Non-standard prompt from R (menu selection, etc.)
                // Pass through R's actual prompt instead of our configured one
                RPrompt::new(r_prompt.to_string(), r_prompt.to_string())
            };

            // Use shell editor when in shell mode (for separate history)
            let editor = if state.prompt_config.is_shell_enabled() {
                &mut state.shell_line_editor
            } else {
                &mut state.line_editor
            };

            // Process R events once before entering the input loop.
            // The idle callback will continue processing events at ~30fps while waiting for input,
            // keeping graphics windows (plot(), help browser) responsive.
            arf_libr::process_r_events();

            // Track whether we're in a non-standard prompt mode (menu selection, etc.)
            let is_menu_prompt = !is_r_command_prompt(r_prompt) && !r_prompt.starts_with('+');

            match editor.read_line(&prompt) {
                Ok(Signal::Success(line)) => {
                    // For non-standard prompts (menus, etc.), pass input directly to R
                    // without any processing (meta commands, shell mode, reprex, autoformat)
                    if is_menu_prompt {
                        return Some(line);
                    }

                    // Check for meta commands first
                    if let Some(result) = process_meta_command(
                        &line,
                        &mut state.prompt_config,
                        &state.config_path,
                        state.config_load_ok,
                        &state.r_history_path,
                        &state.shell_history_path,
                        &state.r_source_status,
                        &mut state.dir_stack,
                    ) {
                        // Clear duration so the previous R command's time
                        // does not persist in the prompt after a meta command.
                        state.prompt_config.clear_command_duration();
                        match result {
                            MetaCommandResult::Handled => {
                                // Command processed, show new prompt
                                continue;
                            }
                            MetaCommandResult::Exit => {
                                state.should_exit = true;
                                return None;
                            }
                            MetaCommandResult::Unknown(cmd) => {
                                arf_println!(
                                    "Unknown command: {}. Type :commands for available commands.",
                                    cmd
                                );
                                continue;
                            }
                            MetaCommandResult::ShellExecuted => {
                                // Shell command was executed, show new prompt
                                continue;
                            }
                            MetaCommandResult::Restart(version) => {
                                // Restart the process, optionally with a new R version
                                restart_process(version.as_deref());
                                // If restart_process returns, it means exec failed
                                // Continue with the current session
                                continue;
                            }
                        }
                    }

                    // Shell mode: execute as shell command instead of R
                    if state.prompt_config.is_shell_enabled() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            // Check if user wants to exit shell mode.
                            // We compare commands as strings because Shell mode doesn't run
                            // a persistent shell process - each command is executed via
                            // `$SHELL -c "command"`. There's no actual shell session to exit,
                            // so we intercept "exit" and "logout" to return to R mode instead
                            // of running them as no-op shell commands.
                            if trimmed == "exit" || trimmed == "logout" {
                                state.prompt_config.set_shell(false);
                                arf_println!("Returned to R mode.");
                                continue;
                            }
                            // Show a hint for cd/pushd/popd since they have no effect
                            // in a subprocess. The command still runs in the shell.
                            if let Some(hint) = meta_command::dir_command_hint(trimmed) {
                                arf_println!("{}", hint);
                            }
                            execute_shell_command(trimmed);
                        }
                        continue;
                    }

                    // In reprex mode, strip lines starting with "#>" (reprex output comments)
                    // This allows users to paste reprex output directly without duplicate output
                    // Keep original for line count calculation in clear_input_lines
                    let (original_line, line) = if state.prompt_config.is_reprex_enabled() {
                        (line.clone(), strip_reprex_output(&line))
                    } else {
                        (line.clone(), line)
                    };

                    // Format code if autoformat is enabled
                    let code = state.prompt_config.maybe_format_code(&line);

                    // In reprex mode, clear the prompt and input lines
                    // Show the (possibly formatted) code
                    // Use original_line for line count since that's what was displayed on terminal
                    if state.prompt_config.is_reprex_enabled() && !code.is_empty() {
                        clear_input_lines(&original_line, &code);
                    }

                    // Record command start time for the {duration} prompt placeholder
                    // Start the spinner to indicate R is evaluating code
                    // The spinner will be stopped when R produces output or the next prompt appears
                    if !code.is_empty() {
                        state.prompt_config.set_command_start();
                        state.prompt_config.start_spinner();
                    }

                    // On Windows, reedline inserts CRLF for newlines in multiline input.
                    // R doesn't accept CR in code, so we need to strip it.
                    // Use the shared strip_cr function from arf_libr.
                    #[cfg(windows)]
                    let code = arf_libr::strip_cr(&code).into_owned();

                    // Return the (possibly formatted) code to R
                    return Some(code);
                }
                Ok(Signal::CtrlC) => {
                    // Clear any visible completion menu before printing ^C
                    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
                    println!("^C");
                    // In shell mode, Ctrl+C returns to R mode
                    if state.prompt_config.is_shell_enabled() {
                        state.prompt_config.set_shell(false);
                        arf_println!("Returned to R mode.");
                        continue;
                    }
                    return Some(String::new());
                }
                Ok(Signal::CtrlD) => {
                    // Clear any visible menu before proceeding
                    let _ = io::stdout().execute(terminal::Clear(ClearType::FromCursorDown));
                    // In shell mode, Ctrl+D returns to R mode (consistent with Ctrl+C)
                    if state.prompt_config.is_shell_enabled() {
                        state.prompt_config.set_shell(false);
                        arf_println!("Returned to R mode.");
                        continue;
                    }
                    state.should_exit = true;
                    return None;
                }
                Err(err) => {
                    eprintln!("Error: {}", err);
                    state.should_exit = true;
                    return None;
                }
            }
        }
    })
}

/// Check if the prompt is R's standard command prompt (top-level).
///
/// Uses R's call stack depth (sys.nframe()) to determine if we're at the top-level
/// or if user code is requesting input (e.g., via readline() or menu()).
///
/// This approach is more robust than heuristics like checking prompt endings,
/// because it detects the actual R evaluation context.
///
/// Returns true if:
/// - We're at the top-level (n_frame == 0) AND not a continuation prompt
///
/// Returns false if:
/// - This is a continuation prompt (starts with '+')
/// - User code is requesting input (n_frame > 0), e.g., readline(), menu()
///
/// Reference: This approach is used by ark (Positron's R kernel).
fn is_r_command_prompt(prompt: &str) -> bool {
    // Continuation prompts (starting with '+') are NOT command prompts
    if prompt.starts_with('+') {
        return false;
    }

    // Use R's call stack depth to detect if we're at top-level
    // n_frame == 0 means top-level prompt
    // n_frame > 0 means user code is requesting input (readline, menu, etc.)
    match arf_harp::r_n_frame() {
        Ok(n_frame) => n_frame == 0,
        Err(_) => {
            // If we can't get n_frame, fall back to heuristic
            // R's default prompt ends with "> ", menu prompts end with ": "
            prompt.ends_with("> ")
        }
    }
}

/// Set up history for a line editor with a specific database path.
///
/// Returns the line editor with history configured (or unchanged if path is None).
/// The history is wrapped with FuzzyHistory to provide fuzzy search capabilities.
fn setup_history(line_editor: Reedline, history_path: Option<std::path::PathBuf>) -> Reedline {
    // Set up SQLite-backed history if we have a path
    if let Some(path) = history_path
        && let Ok(history) = SqliteBackedHistory::with_file(path, None, None)
    {
        // Wrap with FuzzyHistory for fuzzy Ctrl+R search
        let fuzzy_history = FuzzyHistory::new(history);
        return line_editor.with_history(Box::new(fuzzy_history));
    }

    line_editor
}

#[cfg(test)]
mod sponge_tests {
    use super::state::SpongeQueue;
    use reedline::HistoryItemId;

    /// Helper to create a HistoryItemId for testing.
    fn make_id(id: i64) -> HistoryItemId {
        HistoryItemId::new(id)
    }

    /// Helper to run a sequence of commands through SpongeQueue and collect deleted IDs.
    fn run_commands(commands: &[(bool, i64)], delay: usize) -> Vec<HistoryItemId> {
        let mut queue = SpongeQueue::new();
        let mut deleted = Vec::new();

        for &(is_failure, id) in commands {
            let history_id = Some(make_id(id));
            if let Some(id_to_delete) = queue.record_command(is_failure, history_id, delay) {
                deleted.push(id_to_delete);
            }
        }

        deleted
    }

    #[test]
    fn test_sponge_delay_keeps_recent_failures() {
        // With delay=2, failed commands should remain accessible for 2 more commands
        // Command sequence: fail(1), success, success -> fail(1) should be deleted
        let commands = vec![
            (true, 1),  // Failed command with ID 1
            (false, 2), // Success
            (false, 3), // Success -> now queue has 3 entries, purge oldest
        ];

        let deleted = run_commands(&commands, 2);
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0], make_id(1));
    }

    #[test]
    fn test_sponge_delay_zero_deletes_immediately() {
        // With delay=0, failed commands are deleted immediately
        let commands = vec![
            (true, 1), // Failed -> immediately deleted (queue > 0)
        ];

        let deleted = run_commands(&commands, 0);
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0], make_id(1));
    }

    #[test]
    fn test_sponge_success_does_not_delete() {
        // Successful commands never cause deletions directly
        let commands = vec![
            (false, 1), // Success
            (false, 2), // Success
            (false, 3), // Success -> purges None entries
        ];

        let deleted = run_commands(&commands, 2);
        assert!(deleted.is_empty(), "No failures to delete");
    }

    #[test]
    fn test_sponge_multiple_failures_fifo() {
        // Multiple failures should be deleted in FIFO order
        // delay=2: keep 2 entries, delete older ones
        let commands = vec![
            (true, 1),  // fail(1) -> queue: [Some(1)]
            (true, 2),  // fail(2) -> queue: [Some(2), Some(1)]
            (true, 3),  // fail(3) -> queue: [Some(3), Some(2), Some(1)] -> delete 1
            (false, 4), // success -> queue: [None, Some(3), Some(2)] -> delete 2
        ];

        let deleted = run_commands(&commands, 2);
        assert_eq!(deleted.len(), 2);
        assert_eq!(deleted[0], make_id(1)); // First to be deleted
        assert_eq!(deleted[1], make_id(2)); // Second to be deleted
    }

    #[test]
    fn test_sponge_interleaved_success_failure() {
        // Interleaved success/failure pattern
        // delay=3
        let commands = vec![
            (true, 1),  // fail -> [Some(1)]
            (false, 2), // ok   -> [None, Some(1)]
            (true, 3),  // fail -> [Some(3), None, Some(1)]
            (false, 4), // ok   -> [None, Some(3), None, Some(1)] -> delete 1
            (false, 5), // ok   -> [None, None, Some(3), None] -> delete None (no-op)
        ];

        let deleted = run_commands(&commands, 3);
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0], make_id(1));
    }

    #[test]
    fn test_sponge_large_delay_no_deletion() {
        // With a large delay, nothing gets deleted during the session
        let commands = vec![(true, 1), (true, 2), (true, 3), (false, 4), (false, 5)];

        let deleted = run_commands(&commands, 100);
        assert!(deleted.is_empty(), "Delay is larger than command count");
    }

    #[test]
    fn test_sponge_delay_one_keeps_one_command() {
        // delay=1 means keep only the most recent command in queue
        let commands = vec![
            (true, 1),  // fail -> [Some(1)]
            (false, 2), // ok   -> [None, Some(1)] -> delete 1
            (true, 3),  // fail -> [Some(3), None] -> delete None (no-op)
            (true, 4),  // fail -> [Some(4), Some(3)] -> delete 3
        ];

        let deleted = run_commands(&commands, 1);
        assert_eq!(deleted.len(), 2);
        assert_eq!(deleted[0], make_id(1));
        assert_eq!(deleted[1], make_id(3));
    }

    #[test]
    fn test_sponge_realistic_scenario() {
        // Realistic scenario: user typos a command, then fixes it
        // With delay=2, they can use up-arrow to see the failed command for 2 commands
        let commands = vec![
            (true, 100),  // typo: "gti status" -> [Some(100)]
            (false, 101), // fix: "git status" -> [None, Some(100)]
            (false, 102), // continue: "git add ." -> [None, None, Some(100)] -> delete 100
        ];

        let deleted = run_commands(&commands, 2);
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0], make_id(100));
    }

    #[test]
    fn test_sponge_drain_failed_ids() {
        // Test drain_failed_ids for exit cleanup
        let mut queue = SpongeQueue::new();

        // Add some commands with a large delay (no immediate deletion)
        queue.record_command(true, Some(make_id(1)), 100);
        queue.record_command(false, Some(make_id(2)), 100);
        queue.record_command(true, Some(make_id(3)), 100);
        queue.record_command(false, Some(make_id(4)), 100);

        // Drain should return only the failed command IDs
        let drained: Vec<_> = queue.drain_failed_ids().collect();
        assert_eq!(drained.len(), 2);
        // drain_failed_ids returns in FIFO order (oldest first)
        assert_eq!(drained[0], make_id(1));
        assert_eq!(drained[1], make_id(3));

        // Queue should be empty after drain
        assert!(queue.is_empty());
    }
}
