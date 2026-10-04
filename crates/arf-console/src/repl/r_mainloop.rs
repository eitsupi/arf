use super::*;

impl Repl {
    /// Run with R's main loop (run_Rmainloop).
    pub(super) fn run_with_r_mainloop(&self) -> Result<()> {
        let (line_editor, r_history_handle) = self.create_r_line_editor();

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
                history_location: self.history_location.clone(),
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
                error_handler_setup_attempted: false,
            });
        });

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

        if should_install_help_submit_wrapper(self.config.experimental.r_help.viewer) {
            match arf_harp::help_bridge::install_help_submit_wrapper() {
                Ok(arf_harp::help_bridge::HelpSubmitInstallOutcome::Installed) => {
                    log::info!("Native R help pager integration installed");
                }
                Ok(arf_harp::help_bridge::HelpSubmitInstallOutcome::SkippedExistingMethod) => {
                    log::info!(
                        "Native R help pager integration skipped: a custom print method is active"
                    );
                }
                Err(error) => {
                    log::warn!(
                        "Native R help pager integration unavailable; retaining R help output: {error:?}"
                    );
                }
            }
        } else {
            log::debug!("Native R help pager integration disabled by configuration");
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
}

fn should_install_help_submit_wrapper(viewer: super::HelpViewer) -> bool {
    viewer == super::HelpViewer::Auto
}

#[cfg(test)]
mod tests {
    use super::should_install_help_submit_wrapper;
    use crate::config::HelpViewer;

    #[test]
    fn r_help_viewer_does_not_install_native_submit_wrapper() {
        assert!(!should_install_help_submit_wrapper(HelpViewer::R));
        assert!(should_install_help_submit_wrapper(HelpViewer::Auto));
    }
}
