//! REPL (Read-Eval-Print Loop) implementation.

mod banner;
mod editor_setup;
pub(super) mod history;
mod history_runtime;
mod meta_command;
mod pager_ui;
mod prompt;
mod r_mainloop;
mod read_console;
pub(crate) mod reprex;
mod shell;
mod signals;
mod standalone;
pub(crate) mod state;
mod terminal;

use crate::completion::completer::CombinedCompleter;
use crate::completion::menu::{FunctionAwareMenu, StateSyncHistoryMenu};
use crate::completion::shell::ShellCompleter;
use crate::config::{
    AutoSuggestions, Config, ConfigFileInfo, EditorMode, FormatterBackend, HelpViewer,
    ModeIndicatorPosition, RSourceStatus, ReprexMode, ResolvedHistoryLocation,
};
use crate::editor::hinter::RLanguageHinter;
use crate::editor::mode::new_editor_state_ref;
use crate::editor::prompt::PromptFormatter;
use crate::highlighter::{CombinedHighlighter, MetaCommandHighlighter};
use crate::history::HistoryRuntime;
use anyhow::Result;
use crossterm::{ExecutableCommand, style::Stylize};
use nu_ansi_term::{Color, Style};
use reedline::{
    AutoPairs, DefaultHinter, HistorySessionId, IdeMenu, ListMenu, MenuBuilder, Reedline,
    ReedlineMenu, Signal,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;

use crate::editor::validator::RValidator;
use banner::format_banner;
use history::finalize_history;
#[cfg(test)]
#[allow(unused_imports)]
use history::setup_history;
use meta_command::{
    MetaAction, MetaCommandContext, SessionInfoContext, handle_meta_command_result,
    process_meta_command,
};
use prompt::RPrompt;
use read_console::read_console_callback;
use reprex::ReprexRuntime;
use reprex::{clear_input_lines, strip_reprex_output};
use shell::execute_shell_command;
pub(crate) use signals::install_r_interrupt_handler;
#[cfg(unix)]
pub(crate) use signals::restore_default_sigint_handler;
use state::{PendingHistoryContext, PromptRuntimeConfig, ReplState};
use terminal::sync_r_width;

// Thread-local storage for the REPL state.
// This allows the ReadConsole callback to access the line editor.
thread_local! {
    pub(super) static REPL_STATE: RefCell<Option<ReplState>> = const { RefCell::new(None) };
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
    /// Config file path, load status, and selection source (for :info display).
    config_file_info: ConfigFileInfo,
    /// How R was resolved at startup (determines if :switch is available).
    r_source_status: RSourceStatus,
    /// Startup notice for an R source selected by a configured override.
    override_notice: Option<String>,
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
    /// The config file metadata records the path, load status, and source used.
    ///
    /// The `r_source_status` describes how R was resolved at startup,
    /// which determines if features like `:switch` are available.
    /// `override_notice` is shown at startup when a configured override selected R.
    pub fn new(
        config: Config,
        config_file_info: ConfigFileInfo,
        r_source_status: RSourceStatus,
        override_notice: Option<String>,
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
            config_file_info,
            r_source_status,
            override_notice,
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
                self.override_notice.as_deref(),
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
            && let Some(notice) = &self.override_notice
        {
            eprintln!("{notice}");
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
