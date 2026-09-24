use super::outcome::*;

fn id(value: u64) -> CommandId {
    CommandId(value)
}

fn apply(state: Option<CommandLifecycle>, event: CommandEvent) -> CommandReduction {
    reduce_command(state, event)
}

fn accepted(id: CommandId, origin: CommandOrigin) -> CommandReduction {
    apply(None, CommandEvent::Accepted { id, origin })
}

fn awaiting(id: CommandId, origin: CommandOrigin) -> CommandLifecycle {
    let accepted = accepted(id, origin);
    let started = apply(accepted.lifecycle, CommandEvent::Started { id });
    let waiting = apply(started.lifecycle, CommandEvent::AwaitingTopLevel { id });
    waiting
        .lifecycle
        .expect("command should await native outcome")
}

#[test]
fn completed_command_reduces_through_awaiting_and_projects_success_once() {
    let id = id(42);
    let origin = CommandOrigin::VisibleIpc;
    let state = awaiting(id, origin);
    assert_eq!(state.progress, CommandProgress::AwaitingTopLevel);

    let reduced = apply(
        Some(state),
        CommandEvent::NativeTerminal {
            id,
            fact: NativeTerminalFact::Completed,
        },
    );
    let finalized = reduced.finalized.expect("first terminal fact finalizes");
    assert_eq!(finalized.id, id);
    assert_eq!(finalized.origin, origin);
    assert_eq!(
        finalized.consumer_projection(),
        ConsumerProjection {
            prompt: PromptEffect::SetSuccess,
            history: HistoryEffect::Success,
            sponge: SpongeEffect::RecordSuccess,
        }
    );
    assert_eq!(
        reduced.lifecycle.unwrap().progress,
        CommandProgress::Finalized(TerminalOutcome::Native(NativeTerminalFact::Completed))
    );

    let duplicate = apply(
        reduced.lifecycle,
        CommandEvent::NativeTerminal {
            id,
            fact: NativeTerminalFact::Completed,
        },
    );
    assert_eq!(duplicate.finalized, None);
    assert_eq!(
        duplicate.disposition,
        ReductionDisposition::Ignored(IgnoreReason::DuplicateTerminal)
    );
}

#[test]
fn every_native_abort_phase_projects_failure_and_sponge_forget() {
    let id = id(8);
    for phase in [CommandPhase::Parse, CommandPhase::Eval, CommandPhase::Print] {
        let state = awaiting(id, CommandOrigin::User);
        let reduced = apply(
            Some(state),
            CommandEvent::NativeTerminal {
                id,
                fact: NativeTerminalFact::Aborted(phase),
            },
        );
        let finalized = reduced.finalized.unwrap();
        assert_eq!(
            finalized.consumer_projection(),
            ConsumerProjection {
                prompt: PromptEffect::SetFailure,
                history: HistoryEffect::Failure,
                sponge: SpongeEffect::RecordFailure,
            }
        );
    }
}

#[test]
fn cancelled_and_unobserved_outcomes_are_neutral() {
    let neutral = ConsumerProjection {
        prompt: PromptEffect::Keep,
        history: HistoryEffect::Keep,
        sponge: SpongeEffect::NoChange,
    };
    assert_eq!(
        FinalizedCommand {
            id: id(98),
            origin: CommandOrigin::User,
            outcome: TerminalOutcome::Cancelled,
        }
        .consumer_projection(),
        neutral
    );
    assert_eq!(
        FinalizedCommand {
            id: id(97),
            origin: CommandOrigin::User,
            outcome: TerminalOutcome::Native(NativeTerminalFact::Unobserved),
        }
        .consumer_projection(),
        neutral
    );

    let accepted = accepted(id(99), CommandOrigin::Internal);
    let cancelled = apply(accepted.lifecycle, CommandEvent::Cancelled { id: id(99) });
    assert_eq!(
        cancelled.finalized.unwrap().outcome,
        TerminalOutcome::Cancelled
    );
}

#[test]
fn internal_command_outcomes_never_affect_interactive_consumers() {
    for outcome in [
        TerminalOutcome::Native(NativeTerminalFact::Completed),
        TerminalOutcome::Native(NativeTerminalFact::Aborted(CommandPhase::Eval)),
        TerminalOutcome::Native(NativeTerminalFact::Unobserved),
        TerminalOutcome::Cancelled,
    ] {
        assert_eq!(
            FinalizedCommand {
                id: id(3),
                origin: CommandOrigin::Internal,
                outcome,
            }
            .consumer_projection(),
            ConsumerProjection::NEUTRAL
        );
    }
}

#[test]
fn stale_native_outcome_does_not_finalize_new_command() {
    let current = awaiting(id(2), CommandOrigin::User);
    let reduced = apply(
        Some(current),
        CommandEvent::NativeTerminal {
            id: id(1),
            fact: NativeTerminalFact::Aborted(CommandPhase::Eval),
        },
    );
    assert_eq!(reduced.lifecycle, Some(current));
    assert_eq!(reduced.finalized, None);
    assert_eq!(
        reduced.disposition,
        ReductionDisposition::Ignored(IgnoreReason::StaleCommand)
    );
}

#[test]
fn terminal_native_fact_is_rejected_before_awaiting_top_level() {
    let accepted = accepted(id(12), CommandOrigin::User);
    let reduced = apply(
        accepted.lifecycle,
        CommandEvent::NativeTerminal {
            id: id(12),
            fact: NativeTerminalFact::Completed,
        },
    );
    assert_eq!(
        reduced.disposition,
        ReductionDisposition::Rejected(TransitionError::NotAwaitingTopLevel)
    );
    assert_eq!(reduced.lifecycle, accepted.lifecycle);
}

#[test]
fn new_acceptance_replaces_finalized_lifecycle_but_not_a_live_command() {
    let first = awaiting(id(1), CommandOrigin::User);
    let live_reject = apply(
        Some(first),
        CommandEvent::Accepted {
            id: id(2),
            origin: CommandOrigin::Internal,
        },
    );
    assert_eq!(
        live_reject.disposition,
        ReductionDisposition::Rejected(TransitionError::CommandAlreadyActive)
    );

    let finalized = apply(
        Some(first),
        CommandEvent::NativeTerminal {
            id: id(1),
            fact: NativeTerminalFact::Completed,
        },
    );
    let next = apply(
        finalized.lifecycle,
        CommandEvent::Accepted {
            id: id(2),
            origin: CommandOrigin::Internal,
        },
    );
    assert_eq!(next.lifecycle.unwrap().id, id(2));
    assert_eq!(next.disposition, ReductionDisposition::Applied);
}

#[test]
fn cancellation_after_execution_started_is_rejected() {
    let accepted = accepted(id(23), CommandOrigin::VisibleIpc);
    let started = apply(accepted.lifecycle, CommandEvent::Started { id: id(23) });
    let cancelled = apply(started.lifecycle, CommandEvent::Cancelled { id: id(23) });
    assert_eq!(cancelled.finalized, None);
    assert_eq!(
        cancelled.disposition,
        ReductionDisposition::Rejected(TransitionError::CannotCancelStartedCommand)
    );
    assert_eq!(
        cancelled.lifecycle.unwrap().progress,
        CommandProgress::Running(CommandPhase::Parse)
    );
}

#[test]
fn running_command_tracks_parse_eval_and_print_phases() {
    let accepted = accepted(id(24), CommandOrigin::User);
    let started = apply(accepted.lifecycle, CommandEvent::Started { id: id(24) });
    let evaluating = apply(
        started.lifecycle,
        CommandEvent::PhaseChanged {
            id: id(24),
            phase: CommandPhase::Eval,
        },
    );
    let printing = apply(
        evaluating.lifecycle,
        CommandEvent::PhaseChanged {
            id: id(24),
            phase: CommandPhase::Print,
        },
    );
    assert_eq!(
        printing.lifecycle.unwrap().progress,
        CommandProgress::Running(CommandPhase::Print)
    );
}
