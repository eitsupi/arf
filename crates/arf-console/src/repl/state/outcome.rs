#![allow(dead_code)] // This API is introduced ahead of its ReadConsole wiring.

//! Pure command outcome reduction and projection for the REPL consumer.
//!
//! `PendingHistoryContext` remains owned by `ReplState`; this reducer only
//! describes the result policy. The consumer can pair a finalized command ID
//! with that existing context when it applies the history effect.

/// Identity assigned when a host command is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub u64);

/// Origin of a command accepted by the REPL host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandOrigin {
    User,
    VisibleIpc,
    Internal,
}

/// Native phase that can abort while R evaluates a top-level command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandPhase {
    Parse,
    Eval,
    Print,
}

/// Native terminal fact reported for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NativeTerminalFact {
    Completed,
    Aborted(CommandPhase),
    Unobserved,
}

/// Terminal outcome after combining native facts with host cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalOutcome {
    Native(NativeTerminalFact),
    Cancelled,
}

/// Progress for the currently tracked command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandProgress {
    Accepted,
    Running(CommandPhase),
    AwaitingTopLevel,
    Finalized(TerminalOutcome),
}

/// Command identity and reducer state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandLifecycle {
    pub id: CommandId,
    pub origin: CommandOrigin,
    pub progress: CommandProgress,
}

/// Events supplied by the host and the native top-level outcome consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandEvent {
    Accepted {
        id: CommandId,
        origin: CommandOrigin,
    },
    Started {
        id: CommandId,
    },
    PhaseChanged {
        id: CommandId,
        phase: CommandPhase,
    },
    AwaitingTopLevel {
        id: CommandId,
    },
    NativeTerminal {
        id: CommandId,
        fact: NativeTerminalFact,
    },
    Cancelled {
        id: CommandId,
    },
}

/// A command that has reached a terminal outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FinalizedCommand {
    pub id: CommandId,
    pub origin: CommandOrigin,
    pub outcome: TerminalOutcome,
}

/// Errors for events that contradict the active lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionError {
    CommandAlreadyActive,
    NotAccepted,
    NotRunning,
    NotAwaitingTopLevel,
    CannotCancelStartedCommand,
}

/// Events which are harmlessly ignored because they refer to no active command
/// or to a command that has already been superseded/finalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IgnoreReason {
    NoActiveCommand,
    StaleCommand,
    DuplicateTerminal,
}

/// How the reducer handled an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReductionDisposition {
    Applied,
    Ignored(IgnoreReason),
    Rejected(TransitionError),
}

/// Result of one pure state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandReduction {
    pub lifecycle: Option<CommandLifecycle>,
    /// Present only for the event that finalized a command, so consumers apply
    /// prompt/history/sponge effects exactly once.
    pub finalized: Option<FinalizedCommand>,
    pub disposition: ReductionDisposition,
}

/// Prompt effect for the existing last-command status indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptEffect {
    SetSuccess,
    SetFailure,
    Keep,
}

/// History effect corresponding to legacy result codes 0, 1, or no update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryEffect {
    Success, // legacy result 0
    Failure, // legacy result 1
    Keep,    // legacy result None
}

/// Whether the sponge queue should process the command's history row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpongeEffect {
    RecordSuccess,
    RecordFailure,
    NoChange,
}

/// Side-effect policy derived from a terminal outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConsumerProjection {
    pub prompt: PromptEffect,
    pub history: HistoryEffect,
    pub sponge: SpongeEffect,
}

impl FinalizedCommand {
    /// Project a finalized command to prompt, history, and sponge policy.
    ///
    /// Internal evaluations are fail-closed: they never mutate interactive
    /// prompt status, history, or the sponge queue, regardless of native fact.
    pub fn consumer_projection(self) -> ConsumerProjection {
        if self.origin == CommandOrigin::Internal {
            return ConsumerProjection::NEUTRAL;
        }
        match self.outcome {
            TerminalOutcome::Native(NativeTerminalFact::Completed) => ConsumerProjection {
                prompt: PromptEffect::SetSuccess,
                history: HistoryEffect::Success,
                sponge: SpongeEffect::RecordSuccess,
            },
            TerminalOutcome::Native(NativeTerminalFact::Aborted(_)) => ConsumerProjection {
                prompt: PromptEffect::SetFailure,
                history: HistoryEffect::Failure,
                sponge: SpongeEffect::RecordFailure,
            },
            TerminalOutcome::Native(NativeTerminalFact::Unobserved)
            | TerminalOutcome::Cancelled => ConsumerProjection {
                prompt: PromptEffect::Keep,
                history: HistoryEffect::Keep,
                sponge: SpongeEffect::NoChange,
            },
        }
    }
}

impl ConsumerProjection {
    pub const NEUTRAL: Self = Self {
        prompt: PromptEffect::Keep,
        history: HistoryEffect::Keep,
        sponge: SpongeEffect::NoChange,
    };
}

/// Reduce one event against the current command lifecycle.
///
/// The returned `finalized` value is emitted only on the transition to a
/// terminal state. Repeated terminal events leave the finalized lifecycle
/// intact and return no second projection.
pub fn reduce_command(current: Option<CommandLifecycle>, event: CommandEvent) -> CommandReduction {
    let event_id = match event {
        CommandEvent::Accepted { id, .. }
        | CommandEvent::Started { id }
        | CommandEvent::PhaseChanged { id, .. }
        | CommandEvent::AwaitingTopLevel { id }
        | CommandEvent::NativeTerminal { id, .. }
        | CommandEvent::Cancelled { id } => id,
    };

    if let CommandEvent::Accepted { id, origin } = event {
        return match current {
            Some(command) if !matches!(command.progress, CommandProgress::Finalized(_)) => {
                rejected(current, TransitionError::CommandAlreadyActive)
            }
            _ => CommandReduction {
                lifecycle: Some(CommandLifecycle {
                    id,
                    origin,
                    progress: CommandProgress::Accepted,
                }),
                finalized: None,
                disposition: ReductionDisposition::Applied,
            },
        };
    }

    let Some(mut command) = current else {
        return ignored(None, IgnoreReason::NoActiveCommand);
    };
    if command.id != event_id {
        return ignored(Some(command), IgnoreReason::StaleCommand);
    }

    match event {
        CommandEvent::Started { .. } => match command.progress {
            CommandProgress::Accepted => {
                command.progress = CommandProgress::Running(CommandPhase::Parse);
                applied(Some(command))
            }
            CommandProgress::Finalized(_) => {
                ignored(Some(command), IgnoreReason::DuplicateTerminal)
            }
            _ => rejected(Some(command), TransitionError::NotAccepted),
        },
        CommandEvent::PhaseChanged { phase, .. } => match command.progress {
            CommandProgress::Running(_) => {
                command.progress = CommandProgress::Running(phase);
                applied(Some(command))
            }
            CommandProgress::Finalized(_) => {
                ignored(Some(command), IgnoreReason::DuplicateTerminal)
            }
            _ => rejected(Some(command), TransitionError::NotRunning),
        },
        CommandEvent::AwaitingTopLevel { .. } => match command.progress {
            CommandProgress::Running(_) => {
                command.progress = CommandProgress::AwaitingTopLevel;
                applied(Some(command))
            }
            CommandProgress::Finalized(_) => {
                ignored(Some(command), IgnoreReason::DuplicateTerminal)
            }
            _ => rejected(Some(command), TransitionError::NotRunning),
        },
        CommandEvent::NativeTerminal { fact, .. } => match command.progress {
            CommandProgress::AwaitingTopLevel => finalize(command, TerminalOutcome::Native(fact)),
            CommandProgress::Finalized(_) => {
                ignored(Some(command), IgnoreReason::DuplicateTerminal)
            }
            _ => rejected(Some(command), TransitionError::NotAwaitingTopLevel),
        },
        CommandEvent::Cancelled { .. } => match command.progress {
            CommandProgress::Finalized(_) => {
                ignored(Some(command), IgnoreReason::DuplicateTerminal)
            }
            CommandProgress::Accepted => finalize(command, TerminalOutcome::Cancelled),
            CommandProgress::Running(_) | CommandProgress::AwaitingTopLevel => {
                rejected(Some(command), TransitionError::CannotCancelStartedCommand)
            }
        },
        CommandEvent::Accepted { .. } => unreachable!("accepted event handled above"),
    }
}

fn finalize(command: CommandLifecycle, outcome: TerminalOutcome) -> CommandReduction {
    let finalized = FinalizedCommand {
        id: command.id,
        origin: command.origin,
        outcome,
    };
    CommandReduction {
        lifecycle: Some(CommandLifecycle {
            progress: CommandProgress::Finalized(outcome),
            ..command
        }),
        finalized: Some(finalized),
        disposition: ReductionDisposition::Applied,
    }
}

fn applied(lifecycle: Option<CommandLifecycle>) -> CommandReduction {
    CommandReduction {
        lifecycle,
        finalized: None,
        disposition: ReductionDisposition::Applied,
    }
}

fn ignored(lifecycle: Option<CommandLifecycle>, reason: IgnoreReason) -> CommandReduction {
    CommandReduction {
        lifecycle,
        finalized: None,
        disposition: ReductionDisposition::Ignored(reason),
    }
}

fn rejected(lifecycle: Option<CommandLifecycle>, error: TransitionError) -> CommandReduction {
    CommandReduction {
        lifecycle,
        finalized: None,
        disposition: ReductionDisposition::Rejected(error),
    }
}
