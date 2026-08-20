//! REPL (Read-Eval-Print Loop) implementation.

mod banner;
pub(super) mod history;
mod meta_command;
mod pager_ui;
mod prompt;
mod read_console;
pub(crate) mod reprex;
mod shell;
pub(crate) mod state;

use crate::completion::completer::CombinedCompleter;
use crate::completion::menu::{FunctionAwareMenu, StateSyncHistoryMenu};
use crate::completion::shell::ShellCompleter;
use crate::config::{
    AutoSuggestions, Config, ConfigStatus, EditorMode, FormatterBackend, ModeIndicatorPosition,
    RSourceStatus, ReprexMode, history_dir_for_mode,
};
use crate::editor::hinter::RLanguageHinter;
use crate::editor::mode::new_editor_state_ref;
use crate::editor::prompt::PromptFormatter;
use crate::highlighter::{CombinedHighlighter, MetaCommandHighlighter};
use crate::history::HistoryRuntime;
use anyhow::Result;
use crossterm::{
    ExecutableCommand,
    style::Stylize,
    terminal::{self, ClearType},
};
use nu_ansi_term::{Color, Style};
use reedline::{
    DefaultHinter, Emacs, HistorySessionId, IdeMenu, ListMenu, MenuBuilder, Reedline, ReedlineMenu,
    Signal, Vi, default_emacs_keybindings, default_vi_insert_keybindings,
    default_vi_normal_keybindings,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::editor::keybindings::{
    add_auto_match_keybindings, add_common_keybindings, add_key_map_keybindings,
    add_shell_semicolon_keybinding, wrap_edit_mode_with_conditional_rules,
};
use crate::editor::validator::RValidator;
use banner::{format_banner, format_override_line};
use history::finalize_history;
#[cfg(test)]
#[allow(unused_imports)]
use history::setup_history;
use meta_command::{MetaCommandResult, process_meta_command};
use pager_ui::{run_pager_help_browser, run_pager_history_browser, with_ipc_alternate_guard};
use prompt::RPrompt;
use read_console::read_console_callback;
use reprex::ReprexRuntime;
use reprex::{clear_input_lines, strip_reprex_output};
use shell::{execute_shell_command, restart_process};
use state::{PendingHistoryContext, PromptRuntimeConfig, ReplState};

// Thread-local storage for the REPL state.
// This allows the ReadConsole callback to access the line editor.
thread_local! {
    pub(super) static REPL_STATE: RefCell<Option<ReplState>> = const { RefCell::new(None) };
}

/// Last known terminal width for detecting resize.
/// Updated by `sync_r_width()` to avoid redundant R calls.
static LAST_TERMINAL_WIDTH: AtomicU16 = AtomicU16::new(0);

/// Minimum width for R's `options(width)`, matching radian's behavior.
const MIN_R_WIDTH: u16 = 20;

/// Maximum width for R's `options(width)`. R enforces a hard maximum of 10000.
const MAX_R_WIDTH: u16 = 10000;

/// Sync R's `options(width)` with the current terminal width.
///
/// Compares the current terminal columns against the last known width.
/// If changed, updates R's width option. Called both at startup and
/// periodically from the idle callback to handle terminal resize.
fn sync_r_width() {
    let prev = LAST_TERMINAL_WIDTH.load(Ordering::Relaxed);

    let (cols, _) = match terminal::size() {
        Ok(size) => size,
        Err(e) => {
            if prev != 0 {
                // Already have a known width; treat as transient failure.
                log::debug!(
                    "Failed to read terminal size (transient); keeping previous width: {:?}",
                    e
                );
                return;
            }
            // No previous width recorded; fall back to a reasonable default.
            log::debug!(
                "Failed to read terminal size; falling back to default width: {:?}",
                e
            );
            (80, 24)
        }
    };

    let clamped = cols.clamp(MIN_R_WIDTH, MAX_R_WIDTH);
    if prev != clamped {
        let code = format!("options(width = {})", clamped);
        match arf_harp::eval_string_with_visibility(&code) {
            Ok(_) => {
                LAST_TERMINAL_WIDTH.store(clamped, Ordering::Relaxed);
            }
            Err(e) => log::debug!("Failed to set R width option: {:?}", e),
        }
    }
}

/// Install the Ctrl+C handler that forwards interrupts to R.
///
/// Without this, Ctrl+C during R evaluation terminates the process: we set
/// R_SignalHandlers = 0, so R installs no SIGINT handler itself and the
/// default action kills the process (on Unix, R_SelectEx only installs a
/// temporary handler while blocked in select(), leaving a fatal window
/// between calls; on Windows, STATUS_CONTROL_C_EXIT).
/// The handler sets R's interrupt flag (R_interrupts_pending / UserBreak),
/// which R checks periodically (R_CheckUserInterrupt, R_SelectEx) and
/// turns into an interrupt condition via onintr() — but only while R is
/// evaluating: while ReadConsole waits for input there is nothing to
/// interrupt, and a flag observed by the event polling done from the
/// input-waiting loops would make onintr() longjmp through their Rust
/// frames, so the handler drops the signal instead.
///
/// On Unix, call this BEFORE R initialization: startup profiles run inside
/// setup_Rmainloop, so installing later leaves a window where Ctrl+C during
/// a slow .Rprofile kills the process. The interrupt flag pointer is
/// resolved early in initialization (before profiles are evaluated); until
/// then the handler is a no-op. If the flag is still unavailable after
/// initialization, call [`restore_default_sigint_handler`] so Ctrl+C is not
/// swallowed forever. On Windows, profiles are sourced manually after
/// initialization, so call this between the two (gated on flag
/// availability).
pub(crate) fn install_r_interrupt_handler() {
    // Unix: register a SIGINT-only sigaction instead of using ctrlc.
    // The workspace builds ctrlc with the "termination" feature (for
    // headless graceful shutdown), so ctrlc::set_handler would also
    // capture SIGTERM/SIGHUP and an interactive session could no
    // longer be terminated by them.
    #[cfg(unix)]
    {
        use nix::sys::signal;

        extern "C" fn handle_sigint(_signum: std::ffi::c_int) {
            // Async-signal-safe: one atomic load, then an atomic load
            // plus a volatile write. Must not panic or allocate.
            if !arf_libr::is_r_awaiting_console_input() {
                arf_libr::set_r_interrupt_pending();
            }
        }

        // SA_RESTART so blocking syscalls interrupted by the signal
        // are transparently restarted (as ctrlc does).
        let action = signal::SigAction::new(
            signal::SigHandler::Handler(handle_sigint),
            signal::SaFlags::SA_RESTART,
            signal::SigSet::empty(),
        );
        // SAFETY: handle_sigint is async-signal-safe (see above).
        if let Err(e) = unsafe { signal::sigaction(signal::Signal::SIGINT, &action) } {
            log::warn!("Could not set Ctrl+C handler: {e}");
        }
    }

    #[cfg(windows)]
    if let Err(e) = ctrlc::set_handler(|| {
        if !arf_libr::is_r_awaiting_console_input() {
            arf_libr::set_r_interrupt_pending();
        }
    }) {
        log::warn!("Could not set Ctrl+C handler: {e}");
    }
}

/// Restore the default SIGINT disposition (terminate the process).
///
/// Used when R's interrupt flag turns out to be unavailable after R
/// initialization: the handler installed by [`install_r_interrupt_handler`]
/// can never forward interrupts then, and would swallow Ctrl+C forever.
#[cfg(unix)]
pub(crate) fn restore_default_sigint_handler() {
    use nix::sys::signal;

    let action = signal::SigAction::new(
        signal::SigHandler::SigDfl,
        signal::SaFlags::empty(),
        signal::SigSet::empty(),
    );
    // SAFETY: restores the default disposition; no handler code involved.
    if let Err(e) = unsafe { signal::sigaction(signal::Signal::SIGINT, &action) } {
        log::warn!("Could not restore default Ctrl+C handler: {e}");
    }
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
    /// Formatter backend resolved once from the configured selector at startup.
    formatter_backend: Option<FormatterBackend>,
    /// Path to the config file (if specified via --config, or the default XDG path).
    config_path: Option<std::path::PathBuf>,
    /// Status of config file loading (for :info display).
    config_status: ConfigStatus,
    /// How R was resolved at startup (determines if :switch is available).
    r_source_status: RSourceStatus,
    /// R_HOME reported by the running R at startup, if R initialized successfully.
    r_home: Option<std::path::PathBuf>,
    r_initialized: bool,
    prompt_formatter: PromptFormatter,
    /// Session ID for history isolation (shared across R and shell history).
    session_id: Option<HistorySessionId>,
    /// History runtimes prepared before the IPC server advertises this session.
    prepared_r_history: Option<HistoryRuntime>,
    prepared_shell_history: Option<HistoryRuntime>,
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
        config_status: ConfigStatus,
        r_source_status: RSourceStatus,
        r_home: Option<std::path::PathBuf>,
        session_id: Option<HistorySessionId>,
    ) -> Result<Self> {
        let formatter_backend =
            crate::external::formatter::resolve_formatter(config.reprex.formatter);
        // Check if R is initialized
        let r_initialized = arf_libr::r_library().is_ok();

        // Create prompt formatter (caches R version)
        let prompt_formatter = PromptFormatter::new();

        // Set up reprex mode if enabled
        if config.startup.reprex != ReprexMode::Off {
            arf_libr::set_reprex_mode(true, &config.reprex.comment);
        }

        Ok(Repl {
            config,
            formatter_backend,
            config_path,
            config_status,
            r_source_status,
            r_home,
            r_initialized,
            prompt_formatter,
            session_id,
            prepared_r_history: None,
            prepared_shell_history: None,
        })
    }

    /// Initialize both owned runtimes before IPC becomes reachable.
    pub(crate) fn prepare_history(&mut self) {
        if self.prepared_r_history.is_some() {
            return;
        }
        let (r_runtime, shell_runtime) = self.initialize_history_runtimes();
        Self::report_history_runtime("R", &r_runtime);
        Self::report_history_runtime("Shell", &shell_runtime);
        if let Some(store) = r_runtime.store() {
            crate::ipc::set_history_store(store);
        }
        if !r_runtime.is_available() {
            crate::ipc::clear_history_session_id();
        }
        self.prepared_r_history = Some(r_runtime);
        self.prepared_shell_history = Some(shell_runtime);
    }

    /// Build the independent R and shell owners without registering either
    /// one globally. This keeps construction testable and makes global IPC
    /// registration an explicit responsibility of `prepare_history`.
    fn initialize_history_runtimes(&self) -> (HistoryRuntime, HistoryRuntime) {
        let r_runtime = HistoryRuntime::initialize(
            &self.config.history.mode,
            self.r_history_path(),
            self.session_id,
            Some(chrono::Utc::now()),
        );
        let shell_runtime = HistoryRuntime::initialize(
            &self.config.history.mode,
            self.shell_history_path(),
            self.session_id,
            Some(chrono::Utc::now()),
        );
        (r_runtime, shell_runtime)
    }

    fn report_history_runtime(label: &str, runtime: &HistoryRuntime) {
        if let Some(diagnostic) = runtime.startup_warning() {
            eprintln!("Warning: {label} history: {diagnostic}");
            log::warn!("{label} history: {diagnostic}");
        }
    }

    fn prepared_r_history(&self) -> HistoryRuntime {
        self.prepared_r_history
            .clone()
            .expect("history runtimes must be prepared before the REPL starts")
    }

    fn prepared_shell_history(&self) -> HistoryRuntime {
        self.prepared_shell_history
            .clone()
            .expect("history runtimes must be prepared before the REPL starts")
    }

    /// Get the history session ID as an i64 (for IPC).
    pub(crate) fn history_session_id_raw(&self) -> Option<i64> {
        self.prepared_r_history()
            .store()
            .and_then(|store| store.session())
            .map(i64::from)
    }

    pub(crate) fn r_home_for_ipc(&self) -> Option<String> {
        self.r_home.as_ref().map(|path| path.display().to_string())
    }

    /// Get the R history database path based on configuration.
    fn r_history_path(&self) -> Option<std::path::PathBuf> {
        let dir = history_dir_for_mode(&self.config.history.mode);
        dir.map(|d| d.join("r.db"))
    }

    /// Get the Shell history database path based on configuration.
    fn shell_history_path(&self) -> Option<std::path::PathBuf> {
        let dir = history_dir_for_mode(&self.config.history.mode);
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
        // Keep direct callers safe while preserving the invariant that all
        // runtime consumers use the owners registered before IPC startup.
        self.prepare_history();
        // Show startup banner unless disabled
        if self.config.startup.show_banner {
            let banner = format_banner(
                &self.config,
                self.r_initialized,
                self.r_source_status.override_info(),
                self.formatter_backend,
            );
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
        } else if self.r_initialized
            && let Some(info) = self.r_source_status.override_info()
        {
            eprintln!("{}", format_override_line(info));
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
        let r_history_handle = self.prepared_r_history();
        let mut line_editor = r_history_handle.attach_to_editor(line_editor);

        // Set up edit mode (Vi or Emacs) with conditional ':' keybinding
        let editor_state = new_editor_state_ref();
        line_editor = match self.config.editor.mode {
            EditorMode::Vi => {
                let mut insert_keybindings = default_vi_insert_keybindings();
                add_common_keybindings(&mut insert_keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut insert_keybindings);
                }
                if self.config.experimental.shell_semicolon_shortcut {
                    add_shell_semicolon_keybinding(&mut insert_keybindings);
                }
                add_key_map_keybindings(&mut insert_keybindings, &self.config.editor.key_map);
                let vi = Vi::new(insert_keybindings, default_vi_normal_keybindings());
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    vi,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                    self.config.experimental.shell_semicolon_shortcut,
                ))
            }
            EditorMode::Emacs => {
                let mut keybindings = default_emacs_keybindings();
                add_common_keybindings(&mut keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut keybindings);
                }
                if self.config.experimental.shell_semicolon_shortcut {
                    add_shell_semicolon_keybinding(&mut keybindings);
                }
                add_key_map_keybindings(&mut keybindings, &self.config.editor.key_map);
                let emacs = Emacs::new(keybindings);
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    emacs,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                    self.config.experimental.shell_semicolon_shortcut,
                ))
            }
        };

        // Set up combined completer (R + meta commands) if completion is enabled
        // When rig is not enabled, :switch is excluded from completion
        if self.config.completion.enabled {
            let completer = Box::new(CombinedCompleter::with_settings_full(
                self.config.completion.timeout_ms,
                self.config.completion.debounce_ms,
                self.config.completion.auto_paren_limit,
                self.r_source_status.rig_enabled(),
                self.config.experimental.r_completion.fuzzy,
                self.config
                    .experimental
                    .r_completion
                    .package_functions
                    .clone(),
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
        let highlighter = CombinedHighlighter::new(
            self.config.colors.clone(),
            self.config.editor.highlight_matching_bracket,
        )
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
        // Also syncs R's options(width) with terminal size on resize (if enabled).
        //
        // Safety note: This callback runs inside R's ReadConsole callback, but calling
        // R via R_ToplevelExec from here is the standard embedded-R pattern. R explicitly
        // supports this, and radian uses the same approach (setoption() in its inputhook).
        let auto_width = self.config.r.auto_width;
        line_editor = line_editor
            .with_break_signal(crate::ipc::break_signal())
            .with_poll_interval(std::time::Duration::from_millis(33))
            .with_idle_callback(Box::new(move || {
                arf_libr::process_r_events();
                if auto_width {
                    sync_r_width();
                }
                crate::ipc::poll_ipc_requests();
            }));

        // Create shell line editor with separate history
        let (shell_line_editor, shell_history_handle) = self.create_shell_line_editor();

        // The R runtime was registered before the IPC server started; shell
        // history remains a separate owner for shell-mode commands.

        // Create prompt runtime config with unexpanded templates
        // Templates are expanded dynamically in build_main_prompt() to track cwd changes
        let prompt_config = PromptRuntimeConfig::builder(
            self.prompt_formatter.clone(),
            self.config.prompt.format.clone(),
            self.config.prompt.continuation.clone(),
            self.config.prompt.shell_format.clone(),
        )
        .mode_indicator_position(self.config.prompt.mode_indicator)
        .indicators(self.config.prompt.indicators.clone())
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
        // Store state in thread-local
        REPL_STATE.with(|state| {
            *state.borrow_mut() = Some(ReplState {
                line_editor,
                shell_line_editor,
                prompt_config,
                reprex: ReprexRuntime::from_resolved(
                    self.config.startup.reprex,
                    self.config.reprex.comment.clone(),
                    self.config.reprex.formatter,
                    self.formatter_backend,
                ),
                should_exit: false,
                r_prompt_options_ambiguous: false,
                config_path: self.config_path.clone(),
                config_status: self.config_status,
                r_source_status: self.r_source_status.clone(),
                r_home: self.r_home.clone(),
                forget_config: self.config.experimental.history_forget.clone(),
                sponge_queue: state::SpongeQueue::new(),
                dir_stack: Vec::new(),
                // IPC advertises the R runtime's session only; shell history
                // remains separately owned and is not an IPC filter source.
                history_session_id: if r_history_handle.is_available() {
                    self.session_id
                } else {
                    None
                },
                r_history: r_history_handle,
                shell_history: shell_history_handle,
                pending_history_context: PendingHistoryContext::None,
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

        // Sync R's options(width) with the current terminal width.
        // Dynamic resize is handled by the idle callback above.
        if self.config.r.auto_width {
            sync_r_width();
        }

        // Set up the ReadConsole callback
        arf_libr::set_read_console_callback(read_console_callback);

        // Note: the Ctrl+C handler that forwards interrupts to R is installed
        // in main() around R initialization (see install_r_interrupt_handler),
        // so that startup profile evaluation is already covered.

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
                    if let Some(store) = repl_state.r_history.store() {
                        let _ = store.delete(id_to_delete);
                    }
                }
                if let Some(store) = repl_state.r_history.store() {
                    let _ = store.sync();
                }
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
        let history_handle = self.prepared_r_history();
        let mut line_editor = history_handle.attach_to_editor(line_editor);
        // Meta commands use the already-prepared shell owner directly.
        let shell_history_handle = self.prepared_shell_history();
        // Only an available R runtime is advertised for IPC history filtering.
        if !history_handle.is_available() {
            crate::ipc::clear_history_session_id();
        }
        let history_session_id = if history_handle.is_available() {
            self.history_session_id_raw()
        } else {
            None
        };

        // Set up edit mode with conditional ':' keybinding
        let editor_state = new_editor_state_ref();
        line_editor = match self.config.editor.mode {
            EditorMode::Vi => {
                let mut insert_keybindings = default_vi_insert_keybindings();
                add_common_keybindings(&mut insert_keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut insert_keybindings);
                }
                if self.config.experimental.shell_semicolon_shortcut {
                    add_shell_semicolon_keybinding(&mut insert_keybindings);
                }
                add_key_map_keybindings(&mut insert_keybindings, &self.config.editor.key_map);
                let vi = Vi::new(insert_keybindings, default_vi_normal_keybindings());
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    vi,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                    self.config.experimental.shell_semicolon_shortcut,
                ))
            }
            EditorMode::Emacs => {
                let mut keybindings = default_emacs_keybindings();
                add_common_keybindings(&mut keybindings);
                if self.config.editor.auto_match {
                    add_auto_match_keybindings(&mut keybindings);
                }
                if self.config.experimental.shell_semicolon_shortcut {
                    add_shell_semicolon_keybinding(&mut keybindings);
                }
                add_key_map_keybindings(&mut keybindings, &self.config.editor.key_map);
                let emacs = Emacs::new(keybindings);
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    emacs,
                    editor_state.clone(),
                    self.config.editor.auto_match,
                    self.config.experimental.completion_min_chars,
                    self.config.experimental.shell_semicolon_shortcut,
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
        let mode_indicator = match self.config.startup.reprex {
            ReprexMode::Off => None,
            ReprexMode::On if mode_position != ModeIndicatorPosition::None => {
                Some(self.config.prompt.indicators.reprex.clone())
            }
            ReprexMode::Format if mode_position != ModeIndicatorPosition::None => {
                Some(self.config.prompt.indicators.reprex_format.clone())
            }
            _ => None,
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
        let mut standalone_reprex = ReprexRuntime::from_resolved(
            self.config.startup.reprex,
            self.config.reprex.comment.clone(),
            self.config.reprex.formatter,
            self.formatter_backend,
        );
        // Separate dir_stack for standalone mode (R not initialized).
        // The R mainloop path stores its own dir_stack in ReplState.
        // These two paths are mutually exclusive, so no sharing is needed.
        let mut dir_stack: Vec<std::path::PathBuf> = Vec::new();

        loop {
            match line_editor.read_line(&prompt) {
                Ok(Signal::Success(line)) => {
                    let save_outcome = history_handle.receipt_outcome();

                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        // A whitespace-only buffer is still saved by reedline,
                        // so record that it is an ordinary line before skipping.
                        finalize_history(Some(&history_handle), save_outcome, false);
                        continue;
                    }

                    // Process meta commands even when R is not initialized
                    // This allows :switch, :quit, :shell, etc. to work
                    if let Some(result) = process_meta_command(
                        &line,
                        &mut prompt_config,
                        &mut standalone_reprex,
                        &history_handle,
                        &shell_history_handle,
                        &self.r_source_status,
                        &mut dir_stack,
                        history_session_id,
                        self.r_home.as_deref(),
                    ) {
                        finalize_history(Some(&history_handle), save_outcome, true);
                        // Clear duration so the previous R command's time
                        // does not persist in the prompt after a meta command.
                        prompt_config.clear_command_duration();
                        let ctx = SessionInfoContext {
                            prompt_config: &prompt_config,
                            reprex: &standalone_reprex,
                            config_path: &self.config_path,
                            config_status: self.config_status,
                            r_history: &history_handle,
                            shell_history: &shell_history_handle,
                            r_source_status: &self.r_source_status,
                        };
                        match handle_meta_command_result(result, &ctx) {
                            MetaAction::Continue => continue,
                            MetaAction::Exit => {
                                println!("\nGoodbye!");
                                return Ok(());
                            }
                        }
                    }

                    finalize_history(Some(&history_handle), save_outcome, false);

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
                Ok(_) => {
                    // ExternalBreak or future variants: ignore in standalone mode
                    continue;
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
    fn create_shell_line_editor(&self) -> (Reedline, crate::history::HistoryRuntime) {
        // Create shell editor with bracketed paste enabled
        let shell_editor = Reedline::create().use_bracketed_paste(true);

        // Set up SQLite-backed history for Shell mode (separate from R)
        let history_handle = self.prepared_shell_history();
        let mut shell_editor = history_handle.attach_to_editor(shell_editor);

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

        // Set up shell mode completer with path completion if completion is enabled
        if self.config.completion.enabled {
            let completer = Box::new(ShellCompleter::new(
                self.config.experimental.shell_completion.command_names,
            ));
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

        if !self.config.experimental.shell_abbreviations.is_empty() {
            let abbrs: HashMap<String, String> = self
                .config
                .experimental
                .shell_abbreviations
                .clone()
                .into_iter()
                .collect();
            shell_editor = shell_editor.with_abbreviations(abbrs);
        }

        // Set up idle callback to process R events during input waiting.
        // Even in shell mode, R graphics windows may be open and need event processing.
        shell_editor = shell_editor
            .with_poll_interval(std::time::Duration::from_millis(33))
            .with_idle_callback(Box::new(|| {
                arf_libr::process_r_events();
            }));

        (shell_editor, history_handle)
    }
}

/// Result of handling a meta command in the REPL loop.
enum MetaAction {
    /// Continue the REPL loop (show next prompt).
    Continue,
    /// The user requested exit.
    Exit,
}

/// Context for displaying session info in the pager.
struct SessionInfoContext<'a> {
    prompt_config: &'a PromptRuntimeConfig,
    reprex: &'a ReprexRuntime,
    config_path: &'a Option<std::path::PathBuf>,
    config_status: ConfigStatus,
    r_history: &'a HistoryRuntime,
    shell_history: &'a HistoryRuntime,
    r_source_status: &'a RSourceStatus,
}

/// Handle a `MetaCommandResult`, executing pager side effects as needed.
///
/// Returns `MetaAction::Exit` if the user wants to quit, otherwise `MetaAction::Continue`.
/// This is the single place where all `MetaCommandResult` variants are dispatched,
/// shared by both `Repl::run` (pre-R-init loop) and `read_console_callback` (main REPL).
fn handle_meta_command_result(
    result: MetaCommandResult,
    ctx: &SessionInfoContext<'_>,
) -> MetaAction {
    match result {
        MetaCommandResult::Handled | MetaCommandResult::ShellExecuted => MetaAction::Continue,
        MetaCommandResult::Exit => MetaAction::Exit,
        MetaCommandResult::Unknown(cmd) => {
            arf_println!(
                "Unknown command: {}. Type :commands for available commands.",
                cmd
            );
            MetaAction::Continue
        }
        MetaCommandResult::Restart(version) => {
            restart_process(version.as_deref());
            MetaAction::Continue
        }
        MetaCommandResult::ShowHelpBrowser(query) => {
            run_pager_help_browser(&query);
            MetaAction::Continue
        }
        MetaCommandResult::ShowSessionInfo => {
            with_ipc_alternate_guard(|| {
                crate::pager::display_session_info(
                    ctx.prompt_config,
                    ctx.reprex,
                    ctx.config_path,
                    ctx.config_status,
                    ctx.r_history,
                    ctx.shell_history,
                    ctx.r_source_status,
                );
            });
            MetaAction::Continue
        }
        MetaCommandResult::ShowChangelog => {
            with_ipc_alternate_guard(crate::pager::display_changelog);
            MetaAction::Continue
        }
        MetaCommandResult::ShowHistoryBrowser { store, mode } => {
            run_pager_history_browser(&store, mode);
            MetaAction::Continue
        }
        MetaCommandResult::ClearHistory { stores } => {
            let mut cleared_count = 0i64;
            for (name, store) in stores {
                match store.count_all() {
                    Ok(count) if count > 0 => match store.clear() {
                        Ok(()) => cleared_count += count,
                        Err(error) => arf_println!("Failed to clear {} history: {}", name, error),
                    },
                    Ok(_) => {}
                    Err(error) => arf_println!("Failed to read {} history: {}", name, error),
                }
            }
            arf_println!("Cleared {} history entries.", cleared_count);
            MetaAction::Continue
        }
        MetaCommandResult::ShowHistorySchema => {
            if let Err(e) =
                with_ipc_alternate_guard(crate::pager::history_schema::show_schema_pager)
            {
                arf_println!("Error: {}", e);
            }
            MetaAction::Continue
        }
    }
}

#[cfg(test)]
mod history_runtime_tests {
    use super::*;

    #[test]
    fn prepared_r_and_shell_histories_are_distinct_stable_owners() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.history.mode = crate::config::HistoryMode::Persistent {
            dir: Some(temp_dir.path().to_path_buf()),
        };
        let repl = Repl::new(
            config,
            None,
            ConfigStatus::Ok,
            RSourceStatus::Path,
            None,
            Reedline::create_history_session_id(),
        )
        .unwrap();
        let (r_runtime, shell_runtime) = repl.initialize_history_runtimes();
        let r_store = r_runtime.store().unwrap();
        let shell_store = shell_runtime.store().unwrap();
        assert!(!r_store.same_owner(&shell_store));
        assert!(shell_runtime.store().unwrap().same_owner(&shell_store));
    }
}
