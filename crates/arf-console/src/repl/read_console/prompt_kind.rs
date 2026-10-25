//! Classification of the prompts R requests through ReadConsole.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptKind {
    Command,
    UnverifiedCommand,
    Continuation,
    Other,
}

impl PromptKind {
    pub(super) fn is_command(self) -> bool {
        matches!(self, Self::Command | Self::UnverifiedCommand)
    }

    pub(super) fn is_verified_command(self) -> bool {
        matches!(self, Self::Command)
    }

    pub(super) fn is_continuation(self) -> bool {
        matches!(self, Self::Continuation)
    }

    pub(super) fn is_other(self) -> bool {
        matches!(self, Self::Other)
    }
}

/// Classify a ReadConsole prompt once per callback.
///
/// Continuation status comes directly from R's low-level option lookup. For
/// every other prompt, call sys.nframe() once to distinguish a top-level command
/// prompt from input requested by user code (readline(), menu(), browser(), …).
pub(super) fn classify_prompt(prompt: &str, is_continuation: bool) -> PromptKind {
    // R's browser() is primitive and can expose a browser prompt with
    // sys.nframe() == 0, just like the ordinary command prompt. The raw
    // Browse[n]> prompt is the reliable distinction; arf-libr uses the same
    // predicate when validating the prompt options.
    if arf_libr::is_browse_prompt(prompt) {
        return PromptKind::Other;
    }

    if is_continuation {
        return PromptKind::Continuation;
    }

    match arf_harp::r_n_frame() {
        Ok(0) => PromptKind::Command,
        Ok(_) => PromptKind::Other,
        Err(_) => {
            // Preserve the historical command-prompt fallback for lifecycle
            // and IPC behavior, while marking it unverified for operations
            // that require a confirmed top-level prompt.
            if prompt.ends_with("> ") {
                PromptKind::UnverifiedCommand
            } else {
                PromptKind::Other
            }
        }
    }
}

/// Formatter failures only finalize lifecycle state for top-level commands.
/// Continuation prompts belong to the outer command and retain its context.
pub(super) fn formatter_failure_updates_lifecycle(prompt_kind: PromptKind) -> bool {
    prompt_kind.is_command()
}

pub(super) fn should_process_pending_help_requests(
    prompt_kind: PromptKind,
    shell_enabled: bool,
) -> bool {
    prompt_kind.is_verified_command() && !shell_enabled
}

/// Warn once for each transition into an ambiguous prompt-option state.
pub(super) fn should_warn_prompt_ambiguity(was_ambiguous: &mut bool, is_ambiguous: bool) -> bool {
    let should_warn = is_ambiguous && !*was_ambiguous;
    *was_ambiguous = is_ambiguous;
    should_warn
}
