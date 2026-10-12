use super::*;
use crate::editor::keybindings::{
    add_common_keybindings, add_key_map_keybindings, add_shell_semicolon_keybinding,
    wrap_edit_mode_with_conditional_rules,
};
use crate::editor::mode::EditorStateRef;
use crossterm::terminal;
use reedline::{
    EditMode, Emacs, Vi, default_emacs_keybindings, default_vi_insert_keybindings,
    default_vi_normal_keybindings, default_vi_visual_keybindings,
};

/// Build the R-mode edit mode: Vi or Emacs keybindings, wrapped with the
/// context-aware rules driven by the shared editor state.
pub(super) fn build_edit_mode(config: &Config, editor_state: EditorStateRef) -> Box<dyn EditMode> {
    build_edit_mode_with(
        config,
        config.experimental.shell_semicolon_shortcut,
        Some(editor_state),
    )
}

/// Build the shell-mode edit mode: the same Vi or Emacs keybindings as R mode
/// without the `;` shortcut and without conditional rules.
fn build_shell_edit_mode(config: &Config) -> Box<dyn EditMode> {
    build_edit_mode_with(config, false, None)
}

fn build_edit_mode_with(
    config: &Config,
    shell_semicolon_shortcut: bool,
    conditional_state: Option<EditorStateRef>,
) -> Box<dyn EditMode> {
    match config.editor.mode {
        EditorMode::Vi => {
            let mut insert_keybindings = default_vi_insert_keybindings();
            add_common_keybindings(&mut insert_keybindings);
            if shell_semicolon_shortcut {
                add_shell_semicolon_keybinding(&mut insert_keybindings);
            }
            add_key_map_keybindings(&mut insert_keybindings, &config.editor.key_map);
            let vi = Vi::new(
                insert_keybindings,
                default_vi_normal_keybindings(),
                default_vi_visual_keybindings(),
            );
            finish_edit_mode(vi, config, shell_semicolon_shortcut, conditional_state)
        }
        EditorMode::Emacs => {
            let mut keybindings = default_emacs_keybindings();
            add_common_keybindings(&mut keybindings);
            if shell_semicolon_shortcut {
                add_shell_semicolon_keybinding(&mut keybindings);
            }
            add_key_map_keybindings(&mut keybindings, &config.editor.key_map);
            finish_edit_mode(
                Emacs::new(keybindings),
                config,
                shell_semicolon_shortcut,
                conditional_state,
            )
        }
    }
}

fn finish_edit_mode<E: EditMode + 'static>(
    edit_mode: E,
    config: &Config,
    shell_semicolon_shortcut: bool,
    conditional_state: Option<EditorStateRef>,
) -> Box<dyn EditMode> {
    match conditional_state {
        Some(state) => wrap_edit_mode_with_conditional_rules(
            edit_mode,
            state,
            config.experimental.completion_min_chars,
            shell_semicolon_shortcut,
        ),
        None => Box::new(edit_mode),
    }
}

impl Repl {
    /// Create the R-mode editor and attach its history and interactive features.
    pub(super) fn create_r_line_editor(&self) -> (Reedline, crate::history::HistoryRuntime) {
        // Create line editor with bracketed paste enabled
        // This allows detecting paste operations and prevents auto-match from
        // interfering with pasted text (e.g., pasting "()" won't become "())")
        let line_editor = self.configure_auto_pairs(Reedline::create().use_bracketed_paste(true));

        // Set up SQLite-backed history for R mode
        let r_history_handle = self.prepared_r_history();
        let mut line_editor = r_history_handle.attach_to_editor(line_editor);

        // Set up edit mode (Vi or Emacs) with conditional ':' keybinding
        let editor_state = new_editor_state_ref();
        line_editor =
            line_editor.with_edit_mode(build_edit_mode(&self.config, editor_state.clone()));

        // Set up combined completer (R + meta commands) if completion is enabled
        // When :switch is unavailable, it is excluded from completion
        if self.config.completion.enabled {
            let static_formals = arf_harp::completion::StaticFormalsPolicy {
                mode: match self.config.experimental.r_completion.r#static.formals.mode {
                    crate::config::StaticFormalsMode::Off => {
                        arf_harp::completion::StaticFormalsMode::Off
                    }
                    crate::config::StaticFormalsMode::PreferStatic => {
                        arf_harp::completion::StaticFormalsMode::PreferStatic
                    }
                },
                excluded_packages: self
                    .config
                    .experimental
                    .r_completion
                    .r#static
                    .formals
                    .exclusions
                    .packages
                    .clone(),
                excluded_functions: self
                    .config
                    .experimental
                    .r_completion
                    .r#static
                    .formals
                    .exclusions
                    .functions
                    .clone(),
            };
            let completer = Box::new(CombinedCompleter::with_settings_full_and_static_formals(
                self.config.completion.timeout_ms,
                self.config.completion.debounce_ms,
                self.config.completion.auto_paren_limit,
                self.r_source_status.switch_available(),
                self.config.experimental.r_completion.fuzzy,
                self.config
                    .experimental
                    .r_completion
                    .package_functions
                    .clone(),
                static_formals,
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
        (line_editor, r_history_handle)
    }

    /// Create an R language hinter based on config settings.
    ///
    /// Returns `Some(hinter)` if auto_suggestions is enabled, `None` otherwise.
    pub(super) fn create_r_hinter(&self) -> Option<Box<RLanguageHinter>> {
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

    pub(super) fn configure_auto_pairs(&self, line_editor: Reedline) -> Reedline {
        if self.config.editor.auto_match {
            line_editor.with_auto_pairs(AutoPairs::new([
                ('(', ')'),
                ('[', ']'),
                ('{', '}'),
                ('\'', '\''),
                ('"', '"'),
                ('`', '`'),
            ]))
        } else {
            line_editor
        }
    }

    /// Create a shell mode line editor with separate history.
    ///
    /// Shell mode uses a separate SQLite history database from R mode.
    pub(super) fn create_shell_line_editor(&self) -> (Reedline, crate::history::HistoryRuntime) {
        // Create shell editor with bracketed paste enabled
        let shell_editor = Reedline::create().use_bracketed_paste(true);

        // Set up SQLite-backed history for Shell mode (separate from R)
        let history_handle = self.prepared_shell_history();
        let mut shell_editor = history_handle.attach_to_editor(shell_editor);

        // Use same edit mode as R editor
        shell_editor = shell_editor.with_edit_mode(build_shell_edit_mode(&self.config));

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::mode::new_editor_state_ref;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use reedline::{EditCommand, ReedlineEvent, ReedlineRawEvent};

    fn config_with(mode: EditorMode, min_chars: Option<usize>, semicolon: bool) -> Config {
        let mut config = Config::default();
        config.editor.mode = mode;
        config.experimental.completion_min_chars = min_chars;
        config.experimental.shell_semicolon_shortcut = semicolon;
        config
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> ReedlineRawEvent {
        ReedlineRawEvent::try_from(Event::Key(KeyEvent::new(code, modifiers))).unwrap()
    }

    /// Send keys in order to one edit mode instance and collect its events.
    fn events_for(
        mut edit_mode: Box<dyn EditMode>,
        keys: &[(KeyCode, KeyModifiers)],
    ) -> Vec<ReedlineEvent> {
        keys.iter()
            .map(|(code, modifiers)| edit_mode.parse_event(key(*code, *modifiers)))
            .collect()
    }

    fn insert_char(c: char) -> ReedlineEvent {
        ReedlineEvent::Edit(vec![EditCommand::InsertChar(c)])
    }

    fn insert_string(text: &str) -> ReedlineEvent {
        ReedlineEvent::Edit(vec![EditCommand::InsertString(text.to_string())])
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;

    #[test]
    fn emacs_mode_maps_shared_and_key_map_bindings() {
        let config = config_with(EditorMode::Emacs, None, false);
        let events = events_for(
            build_edit_mode(&config, new_editor_state_ref()),
            &[
                (KeyCode::Tab, NONE),
                (KeyCode::Enter, KeyModifiers::SHIFT),
                (KeyCode::Char('-'), KeyModifiers::ALT),
                (KeyCode::Char('p'), KeyModifiers::ALT),
            ],
        );
        assert_eq!(
            events,
            [
                ReedlineEvent::UntilFound(vec![
                    ReedlineEvent::Menu("completion_menu".to_string()),
                    ReedlineEvent::MenuNext,
                ]),
                ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
                insert_string(" <- "),
                insert_string(" |> "),
            ]
        );
    }

    #[test]
    fn vi_mode_maps_insert_mode_bindings() {
        let config = config_with(EditorMode::Vi, None, false);
        let events = events_for(
            build_edit_mode(&config, new_editor_state_ref()),
            &[
                (KeyCode::Tab, NONE),
                (KeyCode::Char('-'), KeyModifiers::ALT),
                (KeyCode::Char('p'), KeyModifiers::ALT),
            ],
        );
        assert_eq!(
            events,
            [
                ReedlineEvent::UntilFound(vec![
                    ReedlineEvent::Menu("completion_menu".to_string()),
                    ReedlineEvent::MenuNext,
                ]),
                insert_string(" <- "),
                insert_string(" |> "),
            ]
        );
    }

    #[test]
    fn colon_triggers_completion_only_at_line_start() {
        for mode in [EditorMode::Vi, EditorMode::Emacs] {
            let config = config_with(mode, None, false);
            let events = events_for(
                build_edit_mode(&config, new_editor_state_ref()),
                &[(KeyCode::Char(':'), NONE), (KeyCode::Char(':'), NONE)],
            );
            assert_eq!(
                events[0],
                ReedlineEvent::Multiple(vec![
                    insert_char(':'),
                    ReedlineEvent::Menu("completion_menu".to_string()),
                ]),
                "{mode:?}: first ':' opens the menu"
            );
            assert_eq!(
                events[1],
                insert_char(':'),
                "{mode:?}: ':' after existing text is a plain character"
            );
        }
    }

    #[test]
    fn semicolon_shortcut_applies_only_when_enabled_and_buffer_is_empty() {
        let keys = [(KeyCode::Char(';'), NONE), (KeyCode::Char(';'), NONE)];
        for mode in [EditorMode::Vi, EditorMode::Emacs] {
            let enabled = events_for(
                build_edit_mode(&config_with(mode, None, true), new_editor_state_ref()),
                &keys,
            );
            assert_eq!(
                enabled[0],
                ReedlineEvent::Multiple(vec![insert_string(":shell"), ReedlineEvent::Submit]),
                "{mode:?}: ';' at an empty buffer enters shell mode"
            );
            assert_eq!(
                enabled[1],
                insert_char(';'),
                "{mode:?}: ';' after existing text is a plain character"
            );

            let disabled = events_for(
                build_edit_mode(&config_with(mode, None, false), new_editor_state_ref()),
                &keys,
            );
            assert_eq!(disabled, [insert_char(';'), insert_char(';')]);
        }
    }

    #[test]
    fn completion_min_chars_appends_a_menu_event_to_character_inserts() {
        let keys = [(KeyCode::Char('a'), NONE)];
        let enabled = events_for(
            build_edit_mode(
                &config_with(EditorMode::Emacs, Some(1), false),
                new_editor_state_ref(),
            ),
            &keys,
        );
        assert_eq!(
            enabled,
            [ReedlineEvent::Multiple(vec![
                insert_char('a'),
                ReedlineEvent::Menu("completion_menu".to_string()),
            ])]
        );

        let disabled = events_for(
            build_edit_mode(
                &config_with(EditorMode::Emacs, None, false),
                new_editor_state_ref(),
            ),
            &keys,
        );
        assert_eq!(disabled, [insert_char('a')]);
    }

    #[test]
    fn shell_edit_mode_has_no_conditional_rules() {
        for mode in [EditorMode::Vi, EditorMode::Emacs] {
            // The shell editor ignores the semicolon shortcut and the
            // completion trigger, and `:` keeps its unconditional binding.
            let config = config_with(mode, Some(1), true);
            let events = events_for(
                build_shell_edit_mode(&config),
                &[
                    (KeyCode::Char(';'), NONE),
                    (KeyCode::Char('a'), NONE),
                    (KeyCode::Char('a'), NONE),
                    (KeyCode::Char(':'), NONE),
                ],
            );
            assert_eq!(events[0], insert_char(';'), "{mode:?}");
            assert_eq!(events[1], insert_char('a'), "{mode:?}");
            assert_eq!(
                events[3],
                ReedlineEvent::Multiple(vec![
                    insert_char(':'),
                    ReedlineEvent::Menu("completion_menu".to_string()),
                ]),
                "{mode:?}: ':' triggers the menu even after text"
            );
        }
    }
}
