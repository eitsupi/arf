use super::lifecycle::outcome_failure_projection;
use super::prompt_kind::{
    PromptKind, classify_prompt, formatter_failure_updates_lifecycle,
    should_process_pending_help_requests, should_warn_prompt_ambiguity,
};

#[test]
fn unavailable_outcome_does_not_project_to_lifecycle_consumers() {
    assert_eq!(
        outcome_failure_projection(arf_libr::CommandOutcome::Unavailable),
        None
    );
    assert_eq!(
        outcome_failure_projection(arf_libr::CommandOutcome::Success),
        Some(false)
    );
    assert_eq!(
        outcome_failure_projection(arf_libr::CommandOutcome::Failure),
        Some(true)
    );
}

#[test]
fn continuation_formatter_failure_keeps_outer_lifecycle_context() {
    assert!(!formatter_failure_updates_lifecycle(
        PromptKind::Continuation
    ));
}

#[test]
fn prepared_help_is_drained_only_at_a_command_prompt_outside_shell_mode() {
    assert!(should_process_pending_help_requests(
        PromptKind::Command,
        false
    ));
    assert!(!should_process_pending_help_requests(
        PromptKind::Command,
        true
    ));
    assert!(!should_process_pending_help_requests(
        PromptKind::UnverifiedCommand,
        false
    ));
    assert!(!should_process_pending_help_requests(
        PromptKind::Continuation,
        false
    ));
    assert!(!should_process_pending_help_requests(
        PromptKind::Other,
        false
    ));
}

#[test]
fn prompt_classification_prioritizes_low_level_continuation_status() {
    assert_eq!(classify_prompt("... ", true, 1), PromptKind::Continuation,);
}

#[test]
fn prompt_classification_falls_back_for_uninitialized_r() {
    let prompt_kind = classify_prompt("> ", false, 1);
    assert_eq!(prompt_kind, PromptKind::UnverifiedCommand);
    assert!(prompt_kind.is_command());
    assert!(!prompt_kind.is_verified_command());
    assert_eq!(classify_prompt("Selection: ", false, 1), PromptKind::Other,);
}

#[test]
fn prompt_ambiguity_warning_is_rearmed_after_distinct_options() {
    let mut was_ambiguous = false;

    assert!(should_warn_prompt_ambiguity(&mut was_ambiguous, true));
    assert!(!should_warn_prompt_ambiguity(&mut was_ambiguous, true));
    assert!(!should_warn_prompt_ambiguity(&mut was_ambiguous, false));
    assert!(should_warn_prompt_ambiguity(&mut was_ambiguous, true));
}
