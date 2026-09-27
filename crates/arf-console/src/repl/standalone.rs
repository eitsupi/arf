use super::*;

impl Repl {
    /// Run without R (standalone mode).
    pub(super) fn run_standalone(&self) -> Result<()> {
        // Create line editor with bracketed paste enabled
        let line_editor = self.configure_auto_pairs(Reedline::create().use_bracketed_paste(true));

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
                if self.config.experimental.shell_semicolon_shortcut {
                    add_shell_semicolon_keybinding(&mut insert_keybindings);
                }
                add_key_map_keybindings(&mut insert_keybindings, &self.config.editor.key_map);
                let vi = Vi::new(
                    insert_keybindings,
                    default_vi_normal_keybindings(),
                    default_vi_visual_keybindings(),
                );
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    vi,
                    editor_state.clone(),
                    self.config.experimental.completion_min_chars,
                    self.config.experimental.shell_semicolon_shortcut,
                ))
            }
            EditorMode::Emacs => {
                let mut keybindings = default_emacs_keybindings();
                add_common_keybindings(&mut keybindings);
                if self.config.experimental.shell_semicolon_shortcut {
                    add_shell_semicolon_keybinding(&mut keybindings);
                }
                add_key_map_keybindings(&mut keybindings, &self.config.editor.key_map);
                let emacs = Emacs::new(keybindings);
                line_editor.with_edit_mode(wrap_edit_mode_with_conditional_rules(
                    emacs,
                    editor_state.clone(),
                    self.config.experimental.completion_min_chars,
                    self.config.experimental.shell_semicolon_shortcut,
                ))
            }
        };

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
            let read_result = line_editor.read_line(&prompt);
            // Keep startup echo suppression through the raw-mode transition.
            // Restore the original cooked mode after reedline returns so its
            // raw-mode handoff has no interval in which input can be echoed.
            if read_result.is_ok()
                && let Err(error) = crate::console_mode::handoff_to_reedline()
            {
                eprintln!("Error: {}", error);
                break;
            }

            match read_result {
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
                            history_location: &self.history_location,
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
}
