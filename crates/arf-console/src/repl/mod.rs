//! REPL (Read-Eval-Print Loop) implementation.

mod banner;
mod editor_setup;
pub(super) mod history;
mod history_runtime;
#[cfg(test)]
mod history_runtime_tests;
mod meta_command;
mod pager_ui;
mod prompt;
mod r_mainloop;
mod read_console;
pub(crate) mod reprex;
mod shell;
mod standalone;
pub(crate) mod state;

use crate::completion::completer::CombinedCompleter;
use crate::completion::menu::{FunctionAwareMenu, StateSyncHistoryMenu};
use crate::completion::shell::ShellCompleter;
use crate::config::{
    AutoSuggestions, Config, ConfigStatus, EditorMode, FormatterBackend, HelpViewer,
    ModeIndicatorPosition, RSourceStatus, ReprexMode, ResolvedHistoryLocation,
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
    AutoPairs, DefaultHinter, Emacs, HistorySessionId, IdeMenu, ListMenu, MenuBuilder, Reedline,
    ReedlineMenu, Signal, Vi, default_emacs_keybindings, default_vi_insert_keybindings,
    default_vi_normal_keybindings, default_vi_visual_keybindings,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::editor::keybindings::{
    add_common_keybindings, add_key_map_keybindings, add_shell_semicolon_keybinding,
    wrap_edit_mode_with_conditional_rules,
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
    /// Effective directory resolved once for R, shell, and schema consumers.
    history_location: ResolvedHistoryLocation,
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
        let history_location = crate::config::resolved_history_location(&config.history.mode);
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
            history_location,
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

    pub(crate) fn r_home_for_ipc(&self) -> Option<String> {
        self.r_home.as_ref().map(|path| path.display().to_string())
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
    history_location: &'a ResolvedHistoryLocation,
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
            if let Err(e) = with_ipc_alternate_guard(|| {
                crate::pager::history_schema::show_schema_pager(ctx.history_location)
            }) {
                arf_println!("Error: {}", e);
            }
            MetaAction::Continue
        }
    }
}
