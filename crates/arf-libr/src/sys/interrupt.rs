use super::read_console::reset_console_generation;
use crate::functions::r_library;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Execution phase used to serialize IPC interrupt requests with R input and
/// evaluation transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptPhase {
    Idle,
    ProtectedEvaluation,
    TopLevelEvaluation,
    Input,
    Finishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterruptExecutionKind {
    UserCode,
    Handler,
}

#[derive(Debug)]
struct InterruptState {
    execution_id: Option<u64>,
    eligible: bool,
    phase: InterruptPhase,
    requested: bool,
    delivered: bool,
    interrupted: bool,
    /// IPC wrote R's interrupt flag for this execution and has not yet seen
    /// whether R consumed it.
    flag_written: bool,
    /// R consumed an interrupt flag written by IPC for this execution.
    flag_consumed: bool,
    /// Reset generation when the execution began, to detect a jump to top level.
    start_generation: u64,
    kind: InterruptExecutionKind,
    deadline_in_force: bool,
    deadline_requested: bool,
    silent_boundaries: bool,
    unavailable_reads: u32,
    protected_depth: usize,
    protected_previous_phase: InterruptPhase,
    /// Re-entrant R evaluations temporarily save the outer execution here.
    stack: Vec<InterruptFrame>,
}

#[derive(Debug, Clone, Copy)]
struct InterruptFrame {
    execution_id: Option<u64>,
    eligible: bool,
    phase: InterruptPhase,
    requested: bool,
    delivered: bool,
    interrupted: bool,
    flag_written: bool,
    flag_consumed: bool,
    start_generation: u64,
    kind: InterruptExecutionKind,
    deadline_in_force: bool,
    deadline_requested: bool,
    silent_boundaries: bool,
    unavailable_reads: u32,
    protected_depth: usize,
    protected_previous_phase: InterruptPhase,
}

impl Default for InterruptState {
    fn default() -> Self {
        Self {
            execution_id: None,
            eligible: false,
            phase: InterruptPhase::Idle,
            requested: false,
            delivered: false,
            interrupted: false,
            flag_written: false,
            flag_consumed: false,
            start_generation: 0,
            kind: InterruptExecutionKind::UserCode,
            deadline_in_force: false,
            deadline_requested: false,
            silent_boundaries: false,
            unavailable_reads: 0,
            protected_depth: 0,
            protected_previous_phase: InterruptPhase::Input,
            stack: Vec::new(),
        }
    }
}

fn snapshot(state: &InterruptState) -> InterruptFrame {
    InterruptFrame {
        execution_id: state.execution_id,
        eligible: state.eligible,
        phase: state.phase,
        requested: state.requested,
        delivered: state.delivered,
        interrupted: state.interrupted,
        flag_written: state.flag_written,
        flag_consumed: state.flag_consumed,
        start_generation: state.start_generation,
        kind: state.kind,
        deadline_in_force: state.deadline_in_force,
        deadline_requested: state.deadline_requested,
        silent_boundaries: state.silent_boundaries,
        unavailable_reads: state.unavailable_reads,
        protected_depth: state.protected_depth,
        protected_previous_phase: state.protected_previous_phase,
    }
}

fn restore(state: &mut InterruptState, frame: InterruptFrame) {
    state.execution_id = frame.execution_id;
    state.eligible = frame.eligible;
    state.phase = frame.phase;
    state.requested = frame.requested;
    state.delivered = frame.delivered;
    state.interrupted = frame.interrupted;
    state.flag_written = frame.flag_written;
    state.flag_consumed = frame.flag_consumed;
    state.start_generation = frame.start_generation;
    state.kind = frame.kind;
    state.deadline_in_force = frame.deadline_in_force;
    state.deadline_requested = frame.deadline_requested;
    state.silent_boundaries = frame.silent_boundaries;
    state.unavailable_reads = frame.unavailable_reads;
    state.protected_depth = frame.protected_depth;
    state.protected_previous_phase = frame.protected_previous_phase;
}

static INTERRUPT_STATE: OnceLock<Mutex<InterruptState>> = OnceLock::new();

fn interrupt_state() -> &'static Mutex<InterruptState> {
    INTERRUPT_STATE.get_or_init(|| Mutex::new(InterruptState::default()))
}

fn lock_interrupt_state() -> std::sync::MutexGuard<'static, InterruptState> {
    interrupt_state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Maximum unavailable console reads allowed in one IPC execution.
const UNAVAILABLE_READ_LIMIT: u32 = 100;

/// Account for an unavailable read without holding a lock while raising R errors.
pub(super) fn unavailable_read_limit_exceeded() -> bool {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_none() {
        return false;
    }
    state.unavailable_reads = state.unavailable_reads.saturating_add(1);
    state.unavailable_reads > UNAVAILABLE_READ_LIMIT
}

/// Unavailable console reads in a forked R worker.
///
/// A worker cannot use the per-execution count above: the interrupt state's
/// mutex may have been held at `fork()` by a parent thread that does not exist
/// in the worker, and the execution it describes belongs to the parent. A
/// worker serves a single evaluation of the parent, so it counts its reads for
/// its whole lifetime instead.
#[cfg(unix)]
static FORKED_UNAVAILABLE_READS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

/// Account for an unavailable read in a forked R worker, without locks.
#[cfg(unix)]
pub(super) fn forked_unavailable_read_limit_exceeded() -> bool {
    // Only R's thread reads the console, so a plain load and store suffice.
    let reads = FORKED_UNAVAILABLE_READS.load(Ordering::Relaxed);
    if reads >= UNAVAILABLE_READ_LIMIT {
        return true;
    }
    FORKED_UNAVAILABLE_READS.store(reads + 1, Ordering::Relaxed);
    false
}

/// Start tracking one IPC user-code evaluation.
pub fn begin_ipc_execution(execution_id: u64, eligible: bool) {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_some() {
        let previous = snapshot(&state);
        state.stack.push(previous);
    }
    state.execution_id = Some(execution_id);
    state.eligible = eligible;
    state.phase = InterruptPhase::Input;
    state.requested = false;
    state.delivered = false;
    state.interrupted = false;
    state.flag_written = false;
    state.flag_consumed = false;
    state.start_generation = reset_console_generation();
    state.kind = InterruptExecutionKind::UserCode;
    state.deadline_in_force = false;
    state.deadline_requested = false;
    state.silent_boundaries = false;
    state.unavailable_reads = 0;
    state.protected_depth = 0;
    state.protected_previous_phase = InterruptPhase::Input;
}

/// Start tracking a handler call. Handler calls remain ineligible for the
/// request-based `interrupt` method; only an explicitly supplied deadline can
/// request a flag.
pub fn begin_handler_ipc_execution(execution_id: u64, deadline_in_force: bool) {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_some() {
        let previous = snapshot(&state);
        state.stack.push(previous);
    }
    state.execution_id = Some(execution_id);
    state.eligible = false;
    state.phase = InterruptPhase::Input;
    state.requested = false;
    state.delivered = false;
    state.interrupted = false;
    state.flag_written = false;
    state.flag_consumed = false;
    state.start_generation = reset_console_generation();
    state.kind = InterruptExecutionKind::Handler;
    state.deadline_in_force = deadline_in_force;
    state.deadline_requested = false;
    state.silent_boundaries = false;
    state.unavailable_reads = 0;
    state.protected_depth = 0;
    state.protected_previous_phase = InterruptPhase::Input;
}

/// Mark a phase transition while preserving an interrupt requested during input.
pub fn set_ipc_execution_phase(phase: InterruptPhase) {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_some() {
        state.phase = phase;
        if matches!(
            phase,
            InterruptPhase::TopLevelEvaluation | InterruptPhase::ProtectedEvaluation
        ) && state.requested
        {
            write_ipc_interrupt_flag(&mut state);
        }
    }
}

/// Prevent new interrupt requests while the R thread finalizes the current
/// evaluation and reads the recorded condition state.
pub fn begin_ipc_execution_finishing() {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_some() {
        state.phase = InterruptPhase::Finishing;
        clear_r_interrupt_pending_unlocked();
    }
}

/// Finish tracking the current IPC evaluation and report whether R actually
/// observed an interrupt or the pending input was discarded.
pub fn finish_ipc_execution() -> bool {
    finish_ipc_execution_outcome().0
}

/// Finish an execution and atomically retrieve its interrupt and deadline state.
pub fn finish_ipc_execution_outcome() -> (bool, bool) {
    let mut state = lock_interrupt_state();
    let interrupted = state.interrupted;
    let deadline_requested = state.deadline_requested;
    state.phase = InterruptPhase::Finishing;
    state.execution_id = None;
    state.eligible = false;
    state.requested = false;
    state.delivered = false;
    state.interrupted = false;
    state.flag_written = false;
    state.flag_consumed = false;
    state.deadline_in_force = false;
    state.deadline_requested = false;
    clear_r_interrupt_pending_unlocked();
    if let Some(frame) = state.stack.pop() {
        restore(&mut state, frame);
        if (state.requested
            || (state.kind == InterruptExecutionKind::Handler && state.deadline_requested))
            && matches!(
                state.phase,
                InterruptPhase::ProtectedEvaluation | InterruptPhase::TopLevelEvaluation
            )
        {
            write_ipc_interrupt_flag(&mut state);
        }
    } else {
        state.phase = InterruptPhase::Idle;
    }
    (interrupted, deadline_requested)
}

/// Enable protected-boundary interrupt delivery for a silent evaluation only.
pub fn enable_ipc_silent_boundaries() {
    let mut state = lock_interrupt_state();
    state.silent_boundaries = true;
}

/// Enter a protected silent-evaluation callback after its jump boundary exists.
pub fn begin_ipc_protected_evaluation() {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_some()
        && state.silent_boundaries
        && state.kind == InterruptExecutionKind::UserCode
        && state.phase != InterruptPhase::Finishing
    {
        if state.protected_depth == 0 {
            state.protected_previous_phase = state.phase;
        }
        state.protected_depth += 1;
        state.phase = InterruptPhase::ProtectedEvaluation;
        if state.requested {
            write_ipc_interrupt_flag(&mut state);
        }
    }
}

/// Restore the outer phase only when the last silent boundary has closed.
pub fn finish_ipc_protected_evaluation() {
    let mut state = lock_interrupt_state();
    if state.silent_boundaries && state.protected_depth > 0 {
        state.protected_depth -= 1;
        if state.protected_depth == 0 {
            clear_r_interrupt_pending_unlocked();
            state.phase = state.protected_previous_phase;
        }
    }
}

/// Enter the interruptible phase of a handler's protected evaluation.
pub fn begin_handler_protected_execution(execution_id: u64) -> bool {
    let mut state = lock_interrupt_state();
    if state.execution_id != Some(execution_id)
        || state.kind != InterruptExecutionKind::Handler
        || !state.deadline_in_force
        || state.phase == InterruptPhase::Finishing
    {
        return false;
    }
    state.phase = InterruptPhase::ProtectedEvaluation;
    if state.deadline_requested {
        write_ipc_interrupt_flag(&mut state);
    }
    true
}

/// Close a handler protected boundary before result extraction. The returned
/// value records whether the deadline request won the request/finish race.
pub fn finish_handler_protected_execution(execution_id: u64) -> bool {
    let mut state = lock_interrupt_state();
    if state.execution_id != Some(execution_id)
        || state.kind != InterruptExecutionKind::Handler
        || state.phase != InterruptPhase::ProtectedEvaluation
    {
        return false;
    }
    let requested = state.deadline_requested;
    state.phase = InterruptPhase::Finishing;
    state.deadline_requested = false;
    clear_r_interrupt_pending_unlocked();
    requested
}

/// Arm a deadline for eligible user code before notifying the timer service.
pub fn enable_ipc_execution_deadline() -> bool {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_some()
        && state.eligible
        && state.kind == InterruptExecutionKind::UserCode
        && state.phase != InterruptPhase::Finishing
    {
        state.deadline_in_force = true;
        return true;
    }
    false
}

/// Register a handler deadline request. A request that arrives during argument
/// conversion is retained, but the R flag is written only while the matching
/// protected callback is active.
pub fn request_ipc_deadline(execution_id: u64) -> bool {
    request_execution_deadline(execution_id, InterruptExecutionKind::Handler)
}

/// Register a user-code deadline using the normal interrupt delivery phases.
pub fn request_ipc_user_code_deadline(execution_id: u64) -> bool {
    request_execution_deadline(execution_id, InterruptExecutionKind::UserCode)
}

fn request_execution_deadline(execution_id: u64, kind: InterruptExecutionKind) -> bool {
    let mut state = lock_interrupt_state();
    if state.execution_id == Some(execution_id)
        && state.kind == kind
        && state.deadline_in_force
        && state.phase != InterruptPhase::Finishing
    {
        state.deadline_requested = true;
        if state.kind == InterruptExecutionKind::UserCode {
            state.requested = true;
        }
        if state.phase == InterruptPhase::ProtectedEvaluation
            || (state.kind == InterruptExecutionKind::UserCode
                && state.phase == InterruptPhase::TopLevelEvaluation)
        {
            write_ipc_interrupt_flag(&mut state);
        }
        return true;
    }
    if let Some(frame) = state
        .stack
        .iter_mut()
        .rev()
        .find(|frame| frame.execution_id == Some(execution_id))
        && frame.kind == kind
        && frame.deadline_in_force
        && frame.phase != InterruptPhase::Finishing
    {
        frame.deadline_requested = true;
        if frame.kind == InterruptExecutionKind::UserCode {
            frame.requested = true;
        }
        return true;
    }
    false
}

/// Retire any active handler deadline during server shutdown.
pub fn cancel_ipc_deadline() {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_some()
        && state.kind == InterruptExecutionKind::Handler
        && state.deadline_requested
        && state.flag_written
    {
        clear_r_interrupt_pending_unlocked();
    }
    if state.kind == InterruptExecutionKind::Handler {
        state.deadline_in_force = false;
        state.deadline_requested = false;
    }
    for frame in &mut state.stack {
        if frame.kind == InterruptExecutionKind::Handler {
            frame.deadline_in_force = false;
            frame.deadline_requested = false;
        }
    }
}

/// Register an interrupt request for the currently active IPC evaluation.
///
/// The ID check, request publication, and R flag write share one lock with
/// phase changes. This function never calls into R while holding the lock.
pub fn request_ipc_interrupt(execution_id: Option<u64>) -> (bool, Option<u64>) {
    let mut state = lock_interrupt_state();
    let Some(active_id) = state.execution_id else {
        return (false, None);
    };
    if !state.eligible
        || state.phase == InterruptPhase::Finishing
        || execution_id.is_some_and(|id| id != active_id)
    {
        return (false, Some(active_id));
    }
    state.requested = true;
    if matches!(
        state.phase,
        InterruptPhase::TopLevelEvaluation | InterruptPhase::ProtectedEvaluation
    ) {
        write_ipc_interrupt_flag(&mut state);
    }
    (true, Some(active_id))
}

/// Consume a request while on the R thread in ReadConsole. The caller must
/// return from its Rust callback before asking R to perform the long jump.
pub fn take_ipc_interrupt_request() -> bool {
    let mut state = lock_interrupt_state();
    if state.phase == InterruptPhase::Input && state.requested {
        state.requested = false;
        state.delivered = true;
        true
    } else {
        false
    }
}

/// Mark that a requested top-level R interrupt was observed.
pub fn mark_ipc_interrupt_observed() {
    let mut state = lock_interrupt_state();
    if state.requested || state.delivered {
        state.interrupted = true;
        state.requested = false;
        state.delivered = false;
    }
}

/// Mark a pending interrupt as effective because the request discarded
/// unconsumed input belonging to the execution.
pub fn mark_ipc_interrupt_discarded() {
    let mut state = lock_interrupt_state();
    if state.requested || state.delivered {
        state.interrupted = true;
        state.requested = false;
        state.delivered = false;
    }
}

/// Pointer to R's interrupt pending flag, set during R initialization.
/// - Unix: points to `R_interrupts_pending` (c_int)
/// - Windows: points to `UserBreak` (Rboolean, c_int-sized)
pub(super) static R_INTERRUPT_FLAG: AtomicPtr<c_int> = AtomicPtr::new(std::ptr::null_mut());

/// Windows only: pointer to `R_interrupts_pending`, set during R initialization.
///
/// The Windows front-end break flag is `UserBreak` (stored in
/// [`R_INTERRUPT_FLAG`]), but when an interrupt arrives while
/// `R_interrupts_suspended` is set, `onintr()` defers it into
/// `R_interrupts_pending` — a different variable. Clearing a stale interrupt
/// on Windows therefore has to reset both.
#[cfg(windows)]
pub(super) static R_DEFERRED_INTERRUPT_FLAG: AtomicPtr<c_int> =
    AtomicPtr::new(std::ptr::null_mut());

/// Number of active `r_read_console` calls waiting for console input.
///
/// The Ctrl+C handler consults this to drop interrupts that arrive while no
/// R computation is running: if R's interrupt flag were set at that time,
/// the event polling done from the input-waiting loops (reedline idle
/// callback, pagers) could observe it and run `onintr()`, which longjmps to
/// R's top level straight through the Rust frames of those loops.
///
/// A counter rather than a flag because ReadConsole can nest: R code run
/// from the event polling of an outer read (e.g. a handler calling
/// `readline()`) enters `r_read_console` again, and the inner guard's drop
/// must not unmark the still-active outer read.
pub(super) static R_AWAITING_CONSOLE_INPUT: AtomicUsize = AtomicUsize::new(0);

/// RAII guard that sets `R_interrupts_suspended = TRUE` and restores the
/// previous value on drop.
///
/// While suspended, R's `onintr()` defers the interrupt (it re-sets
/// `R_interrupts_pending` and returns) instead of longjmping to R's top
/// level. This makes it safe to call R's event machinery from Rust
/// input-waiting loops even if a SIGINT handler on another thread sets the
/// interrupt flag at any point during the call — closing the race that the
/// [`R_AWAITING_CONSOLE_INPUT`] gate and the entry clears only narrow.
///
/// Unlike R's `END_SUSPEND_INTERRUPTS`, the drop deliberately does NOT call
/// `Rf_onintr()` for an interrupt that arrived while suspended: firing it
/// would longjmp through the Rust caller. Instead, the drop clears the
/// deferred pending flag while still suspended and only then restores the
/// previous suspension state, so a deferred interrupt can neither fire here
/// nor leak into the next R evaluation (interrupts make no sense at the
/// prompt). On Windows this matters doubly: onintr() defers into
/// `R_interrupts_pending`, which is a different variable from `UserBreak`.
///
/// `R_interrupts_suspended` is not part of R's documented API (see the
/// comment on `RLibrary::r_interrupts_suspended`); when the symbol is
/// unavailable the guard is a no-op, reverting to the narrowed-race behavior.
///
/// Known residual (accepted): a SIGINT handler on another thread that
/// observed `R_AWAITING_CONSOLE_INPUT == false` just before the transition
/// can still write the interrupt flag after the drop's clear, leaving a
/// stale flag that interrupts the start of the next evaluation once. This
/// is benign as long as suspension is active (no longjmp through Rust
/// frames; on builds where the symbol is unavailable the no-op guard leaves
/// the narrowed crash race described above) and requires a handler preempted
/// inside a several-instruction window plus input submitted within one idle
/// tick. Fully eliminating it would mean
/// blocking SIGINT on all threads and receiving it via sigwait on a
/// dedicated thread that shares a mutex with the ReadConsole transitions —
/// a redesign that is not worth it for a benign one-off; see the task
/// tracker for the design sketch if it ever becomes necessary.
struct SuspendRInterruptsGuard {
    ptr: *mut c_int,
    old: c_int,
}

impl SuspendRInterruptsGuard {
    fn new(ptr: *mut c_int) -> Self {
        let old = if ptr.is_null() {
            0
        } else {
            // SAFETY: ptr points to R's global variable, valid for the
            // process lifetime. Volatile access matches how the SIGINT
            // handler touches R globals.
            unsafe {
                let old = std::ptr::read_volatile(ptr);
                std::ptr::write_volatile(ptr, 1);
                old
            }
        };
        Self { ptr, old }
    }
}

impl Drop for SuspendRInterruptsGuard {
    fn drop(&mut self) {
        // Drop any interrupt deferred while suspended, before lifting the
        // suspension: after this point no R code runs that could convert or
        // defer flags, so the clear cannot race with R itself.
        clear_r_interrupt_pending();

        if !self.ptr.is_null() {
            // SAFETY: see SuspendRInterruptsGuard::new.
            unsafe {
                std::ptr::write_volatile(self.ptr, self.old);
            }
        }
    }
}

/// RAII guard that marks [`R_AWAITING_CONSOLE_INPUT`] for the duration of a
/// `r_read_console` call, covering every return path.
pub(super) struct AwaitConsoleInputGuard;

impl AwaitConsoleInputGuard {
    pub(super) fn new() -> Self {
        R_AWAITING_CONSOLE_INPUT.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

/// Enter a ReadConsole callback before any R calls or input callbacks run.
/// The transition and stale flag clear serialize with IPC interrupt requests.
pub fn begin_read_console_input() -> (Option<u64>, InterruptPhase) {
    let mut state = lock_interrupt_state();
    let previous = (state.execution_id, state.phase);
    if state.execution_id.is_some() {
        observe_interrupt_outcome(&mut state, previous.1);
        state.phase = InterruptPhase::Input;
    }
    clear_r_interrupt_pending_unlocked();
    previous
}

pub(super) fn finish_read_console_input(
    previous: (Option<u64>, InterruptPhase),
    supplied_input: bool,
) {
    let mut state = lock_interrupt_state();
    if state.execution_id.is_none() {
        return;
    }
    state.phase = if state.execution_id == previous.0 {
        previous.1
    } else if supplied_input {
        InterruptPhase::TopLevelEvaluation
    } else {
        InterruptPhase::Input
    };
    if (state.requested
        || (state.kind == InterruptExecutionKind::Handler && state.deadline_requested))
        && matches!(
            state.phase,
            InterruptPhase::ProtectedEvaluation | InterruptPhase::TopLevelEvaluation
        )
    {
        write_ipc_interrupt_flag(&mut state);
    }
}

/// Temporarily allow SIGINT to reach an R evaluation initiated while
/// ReadConsole is active.
///
/// Interactive evaluation normally happens after the ReadConsole callback has
/// returned, so `R_AWAITING_CONSOLE_INPUT` is zero. A small number of prompt
/// actions evaluate R while the callback remains on the stack. In that case
/// this guard lets the regular R interrupt handler deliver Ctrl+C to a
/// protected top-level evaluation. It must be held only outside the
/// `R_ToplevelExec` callback so R's longjmp cannot skip this Rust destructor.
pub struct ConsoleEvaluationInterruptGuard {
    previous_count: usize,
    _not_send_or_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ConsoleEvaluationInterruptGuard {
    /// Temporarily mark ReadConsole as not waiting for input.
    ///
    /// # Safety
    /// The caller must be on R's main thread and must keep this guard outside
    /// any R top-level jump callback. The guarded work must enter R only
    /// through protected evaluation, so a longjmp cannot skip this guard's
    /// destructor. Guards must be scoped in LIFO order, without input-waiting
    /// loops or event polling while held.
    pub unsafe fn new() -> Self {
        let previous_count = R_AWAITING_CONSOLE_INPUT.swap(0, Ordering::AcqRel);
        Self {
            previous_count,
            _not_send_or_sync: std::marker::PhantomData,
        }
    }
}

impl Drop for ConsoleEvaluationInterruptGuard {
    fn drop(&mut self) {
        // Restore the input-waiting state before clearing flags. A concurrent
        // signal that observes the restored count is ignored by the handler;
        // clear flags from the protected evaluation before returning to input.
        // The preexisting race with a signal handler delayed between its
        // counter read and flag write can still leave one stale interrupt.
        R_AWAITING_CONSOLE_INPUT.store(self.previous_count, Ordering::Release);
        clear_r_interrupt_pending();
    }
}

impl Drop for AwaitConsoleInputGuard {
    fn drop(&mut self) {
        R_AWAITING_CONSOLE_INPUT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Process R events.
///
/// This calls R's event processing functions to handle:
/// - Graphics window events (X11, Windows GDI, etc.)
/// - User interrupts
/// - Other system events
///
/// On Unix, this also runs input handlers for background tasks.
///
/// This function should be called periodically while waiting for user input
/// to keep R's interactive windows responsive. arf does so from reedline's
/// idle callback in the interactive REPL and from the polling loop in
/// headless mode.
///
/// # Safety
/// R must be initialized before calling this function.
pub fn process_r_events() {
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };

    // Drop any pending interrupt before calling into R's event machinery.
    // This function is only called while no R computation is running: from
    // Rust input-waiting code (the reedline idle callback, the headless
    // loop, and once just before read_line()), so there is nothing to
    // interrupt. If the flag were left set, an interrupt observed here (by
    // R_ProcessEvents on Windows or R_checkActivity/R_runHandlers on Unix)
    // would call onintr(), which longjmps to R's top level straight through
    // the Rust frames of the caller (undefined behavior; in practice it
    // leaks a RefCell borrow and the session exits on the next
    // ReadConsole).
    clear_r_interrupt_pending();

    // Suspend interrupts for the duration of the call so that a flag set
    // concurrently (by a SIGINT handler running on another thread) cannot
    // trigger that longjmp either. See SuspendRInterruptsGuard.
    let _suspend_guard = SuspendRInterruptsGuard::new(lib.r_interrupts_suspended);

    unsafe {
        // Call R_ProcessEvents - this is the main event processing function
        (lib.r_processevents)();

        // Platform-specific additional event processing
        #[cfg(unix)]
        {
            // On Unix, also check for and run input handlers
            // This handles things like httpuv background requests
            if !lib.r_inputhandlers.is_null() {
                let what = (lib.r_checkactivity)(0, 1);
                if !what.is_null() {
                    (lib.r_runhandlers)(*lib.r_inputhandlers, what);
                }
            }
        }
    }
}

/// Process R events behind an R top-level jump boundary.
///
/// The callback payload contains only a raw library pointer and the callback
/// frame owns no Rust values with destructors, so an R error cannot unwind
/// through Rust-owned state.
pub fn process_r_events_protected() -> bool {
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return false,
    };
    clear_r_interrupt_pending();
    let suspend = SuspendRInterruptsGuard::new(lib.r_interrupts_suspended);
    let mut payload = lib as *const crate::functions::RLibrary as *mut std::ffi::c_void;
    let completed = unsafe {
        (lib.r_toplevelexec)(
            Some(process_r_events_callback),
            (&mut payload as *mut *mut std::ffi::c_void).cast(),
        )
    } != 0;
    drop(suspend);
    completed
}

unsafe extern "C-unwind" fn process_r_events_callback(payload: *mut std::ffi::c_void) {
    let library_ptr =
        unsafe { *(payload.cast::<*mut std::ffi::c_void>()) } as *const crate::functions::RLibrary;
    if library_ptr.is_null() {
        return;
    }
    let lib = unsafe { &*library_ptr };
    unsafe {
        (lib.r_processevents)();
        #[cfg(unix)]
        if !lib.r_inputhandlers.is_null() {
            let what = (lib.r_checkactivity)(0, 1);
            if !what.is_null() {
                (lib.r_runhandlers)(*lib.r_inputhandlers, what);
            }
        }
    }
}

/// Set R's interrupt pending flag to request computation interruption.
///
/// This is async-signal-safe: only performs an atomic load and a volatile write.
/// On Unix, sets `R_interrupts_pending = 1`.
/// On Windows, sets `UserBreak = TRUE` (1).
pub fn set_r_interrupt_pending() {
    set_r_interrupt_pending_unlocked();
}

/// Write R's interrupt flag on behalf of the IPC request and remember that the
/// flag is ours, so a later zero value can be read as consumption by R.
fn write_ipc_interrupt_flag(state: &mut InterruptState) {
    state.flag_written = true;
    set_r_interrupt_pending_unlocked();
}

/// Whether every interrupt flag that R uses to hold an interrupt is zero.
///
/// R's `onintr()` resets `R_interrupts_pending` when it handles an interrupt
/// (Windows also consumes `UserBreak` from `R_ProcessEvents`), whereas while
/// interrupts are suspended it re-defers the interrupt into
/// `R_interrupts_pending`. A zero flag after IPC wrote it therefore means R
/// took the interrupt. Returns `false` when the flag location is unknown.
fn r_interrupt_flags_clear() -> bool {
    let ptr = R_INTERRUPT_FLAG.load(Ordering::Acquire);
    if ptr.is_null() {
        return false;
    }
    // SAFETY: ptr points to R's global variable, valid for the process lifetime.
    let clear = unsafe { std::ptr::read_volatile(ptr) } == 0;
    #[cfg(windows)]
    {
        let deferred = R_DEFERRED_INTERRUPT_FLAG.load(Ordering::Acquire);
        if !deferred.is_null() {
            // SAFETY: same as above.
            return clear && unsafe { std::ptr::read_volatile(deferred) } == 0;
        }
    }
    clear
}

/// Decide at ReadConsole entry, before the stale-flag clear, whether an
/// interrupt requested over IPC stopped the execution. Called with the state
/// lock held; `previous_phase` is the phase before this entry.
///
/// A top-level evaluation counts as interrupted when R consumed the flag IPC
/// wrote and a jump to top level happened since the execution began. An
/// interrupt that arf delivered itself with `Rf_onintr()` from a prompt counts
/// when that was followed by such a jump.
fn observe_interrupt_outcome(state: &mut InterruptState, previous_phase: InterruptPhase) {
    // Silent user code can catch an interrupt and request console input before
    // leaving its protected boundary. Do not deliver the same request again.
    if (previous_phase == InterruptPhase::TopLevelEvaluation
        || (previous_phase == InterruptPhase::ProtectedEvaluation
            && state.kind == InterruptExecutionKind::UserCode))
        && state.flag_written
    {
        state.flag_written = false;
        if r_interrupt_flags_clear() {
            state.flag_consumed = true;
            state.requested = false;
        }
    }
    let jumped = reset_console_generation() != state.start_generation;
    if jumped && (state.flag_consumed || state.delivered) {
        state.interrupted = true;
        state.delivered = false;
    }
}

fn set_r_interrupt_pending_unlocked() {
    let ptr = R_INTERRUPT_FLAG.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: ptr points to R's global variable, valid for the process lifetime.
        // Volatile write is async-signal-safe and prevents compiler elision.
        unsafe {
            std::ptr::write_volatile(ptr, 1);
        }
    }
}

/// Returns whether the R interrupt flag pointer is available.
///
/// When this returns `false`, [`set_r_interrupt_pending`] is a no-op.
pub fn is_r_interrupt_flag_available() -> bool {
    !R_INTERRUPT_FLAG.load(Ordering::Acquire).is_null()
}

/// Returns whether R is currently inside `ReadConsole` waiting for input.
///
/// This is async-signal-safe (a single atomic load). The Ctrl+C handler uses
/// it to drop interrupts that arrive while no R computation is running; see
/// [`R_AWAITING_CONSOLE_INPUT`] for why setting the flag then is unsafe.
pub fn is_r_awaiting_console_input() -> bool {
    R_AWAITING_CONSOLE_INPUT.load(Ordering::Acquire) > 0
}

/// Clear R's interrupt pending flag.
///
/// Called at the start of every `ReadConsole` invocation (including nested
/// prompts such as `readline()`, `browser()`, etc.) to prevent stale
/// interrupt flags from triggering on the next evaluation.
///
/// On Windows this clears both `UserBreak` (the front-end break flag) and
/// `R_interrupts_pending` (where `onintr()` defers an interrupt that arrives
/// while `R_interrupts_suspended` is set). On Unix they are the same flag.
pub(super) fn clear_r_interrupt_pending() {
    let _state = lock_interrupt_state();
    clear_r_interrupt_pending_unlocked();
}

fn clear_r_interrupt_pending_unlocked() {
    let ptr = R_INTERRUPT_FLAG.load(Ordering::Acquire);
    if !ptr.is_null() {
        unsafe {
            std::ptr::write_volatile(ptr, 0);
        }
    }

    #[cfg(windows)]
    {
        let ptr = R_DEFERRED_INTERRUPT_FLAG.load(Ordering::Acquire);
        if !ptr.is_null() {
            unsafe {
                std::ptr::write_volatile(ptr, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AwaitConsoleInputGuard, ConsoleEvaluationInterruptGuard, is_r_awaiting_console_input,
    };
    use std::os::raw::c_int;
    use std::sync::atomic::Ordering;

    /// Serializes tests that use the process-wide IPC interrupt state.
    static STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn user_code_deadlines_are_scoped_and_finish_atomically() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::begin_ipc_execution(9200, false);
        assert!(!super::enable_ipc_execution_deadline());
        assert!(!super::request_ipc_user_code_deadline(9200));
        assert_eq!(super::finish_ipc_execution_outcome(), (false, false));

        super::begin_ipc_execution(9201, true);
        assert!(super::enable_ipc_execution_deadline());
        assert!(!super::request_ipc_deadline(9201));
        assert_eq!(super::request_ipc_interrupt(Some(9201)), (true, Some(9201)));
        super::begin_ipc_execution(9202, true);
        assert!(super::enable_ipc_execution_deadline());
        assert!(super::request_ipc_user_code_deadline(9201));
        assert!(super::request_ipc_user_code_deadline(9202));
        assert!(super::take_ipc_interrupt_request());
        super::mark_ipc_interrupt_observed();
        assert_eq!(super::finish_ipc_execution_outcome(), (true, true));
        assert!(super::take_ipc_interrupt_request());
        super::mark_ipc_interrupt_observed();
        super::begin_ipc_execution_finishing();
        assert!(!super::request_ipc_user_code_deadline(9201));
        assert_eq!(super::finish_ipc_execution_outcome(), (true, true));
        assert!(!super::request_ipc_user_code_deadline(9201));

        super::begin_ipc_execution(9203, true);
        assert!(super::enable_ipc_execution_deadline());
        super::begin_ipc_execution_finishing();
        assert!(!super::request_ipc_user_code_deadline(9203));
        assert_eq!(super::finish_ipc_execution_outcome(), (false, false));
        super::begin_ipc_execution(9204, true);
        assert!(super::enable_ipc_execution_deadline());
        assert!(super::request_ipc_user_code_deadline(9204));
        // A request alone is not evidence that R was interrupted.
        assert_eq!(super::finish_ipc_execution_outcome(), (false, true));
        super::begin_ipc_execution(9205, true);
        assert_eq!(super::finish_ipc_execution_outcome(), (false, false));
    }

    #[test]
    fn unavailable_reads_follow_nested_execution_budgets() {
        let _lock = STATE_TEST_LOCK.lock().unwrap();
        assert!(!super::unavailable_read_limit_exceeded());
        super::begin_ipc_execution(9200, true);
        for _ in 0..99 {
            assert!(!super::unavailable_read_limit_exceeded());
        }
        super::begin_handler_ipc_execution(9201, false);
        for _ in 0..100 {
            assert!(!super::unavailable_read_limit_exceeded());
        }
        assert!(super::unavailable_read_limit_exceeded());
        assert!(super::unavailable_read_limit_exceeded());
        super::finish_ipc_execution();
        assert!(!super::unavailable_read_limit_exceeded());
        assert!(super::unavailable_read_limit_exceeded());
        super::lock_interrupt_state().unavailable_reads = u32::MAX;
        assert!(super::unavailable_read_limit_exceeded());
        super::finish_ipc_execution();
        assert!(!super::unavailable_read_limit_exceeded());
    }

    #[test]
    fn protected_evaluation_restores_nested_console_waits() {
        assert!(!is_r_awaiting_console_input());
        let outer = AwaitConsoleInputGuard::new();
        let inner = AwaitConsoleInputGuard::new();
        assert!(is_r_awaiting_console_input());
        {
            // SAFETY: this test runs no R code or event polling. It checks the
            // input-count transitions with guards scoped in LIFO order.
            let _evaluation = unsafe { ConsoleEvaluationInterruptGuard::new() };
            assert!(!is_r_awaiting_console_input());
            {
                let _nested_read = AwaitConsoleInputGuard::new();
                assert!(is_r_awaiting_console_input());
            }
            assert!(!is_r_awaiting_console_input());
        }
        assert!(is_r_awaiting_console_input());
        drop(inner);
        assert!(is_r_awaiting_console_input());
        drop(outer);
        assert!(!is_r_awaiting_console_input());
    }

    #[test]
    fn ipc_interrupt_requests_are_scoped_to_one_execution_and_consumed_once() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::begin_ipc_execution(7001, true);
        assert_eq!(
            super::request_ipc_interrupt(Some(7000)),
            (false, Some(7001))
        );
        assert_eq!(super::request_ipc_interrupt(Some(7001)), (true, Some(7001)));
        assert!(super::take_ipc_interrupt_request());
        assert!(!super::take_ipc_interrupt_request());
        assert!(!super::finish_ipc_execution());

        // EOF from a nested read can resume evaluation. An event callback
        // nested inside an input loop must instead preserve that input phase.
        super::begin_ipc_execution(7004, true);
        super::set_ipc_execution_phase(super::InterruptPhase::ProtectedEvaluation);
        let previous = super::begin_read_console_input();
        super::finish_read_console_input(previous, false);
        assert_eq!(
            super::lock_interrupt_state().phase,
            super::InterruptPhase::ProtectedEvaluation
        );
        super::set_ipc_execution_phase(super::InterruptPhase::Input);
        let previous = super::begin_read_console_input();
        super::finish_read_console_input(previous, true);
        assert_eq!(
            super::lock_interrupt_state().phase,
            super::InterruptPhase::Input
        );
        assert!(!super::finish_ipc_execution());
        super::begin_ipc_execution(7003, true);
        assert_eq!(super::request_ipc_interrupt(None), (true, Some(7003)));
        assert!(super::take_ipc_interrupt_request());
        super::mark_ipc_interrupt_discarded();
        assert!(super::finish_ipc_execution());

        super::begin_ipc_execution(7002, true);
        assert_eq!(
            super::request_ipc_interrupt(Some(7001)),
            (false, Some(7002))
        );
        assert!(!super::take_ipc_interrupt_request());
        assert!(!super::finish_ipc_execution());
    }

    /// Run one top-level IPC evaluation against a fake R interrupt flag and
    /// report whether the final ReadConsole entry classifies it as interrupted.
    ///
    /// `r_consumes` models R handling the flag, `r_jumps` models a jump to
    /// top level during the evaluation.
    fn classify_top_level_evaluation(
        flag: *mut c_int,
        request: bool,
        r_consumes: bool,
        r_jumps: bool,
    ) -> bool {
        super::begin_ipc_execution(8000, true);
        super::set_ipc_execution_phase(super::InterruptPhase::TopLevelEvaluation);
        if request {
            assert_eq!(super::request_ipc_interrupt(None), (true, Some(8000)));
            // SAFETY: flag points to a leaked c_int owned by the test.
            assert_eq!(unsafe { *flag }, 1, "IPC must write the interrupt flag");
        }
        if r_consumes {
            unsafe { *flag = 0 };
        }
        if r_jumps {
            // SAFETY: the reset callback only increments an atomic counter.
            unsafe { super::super::read_console::r_reset_console() };
        }
        let previous = super::begin_read_console_input();
        assert_eq!(unsafe { *flag }, 0, "entry must clear any remaining flag");
        super::finish_read_console_input(previous, false);
        super::finish_ipc_execution()
    }

    #[test]
    fn top_level_interrupt_requires_flag_consumption_and_a_jump() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flag: *mut c_int = Box::leak(Box::new(0));
        super::R_INTERRUPT_FLAG.store(flag, Ordering::Release);

        // R consumed the flag and unwound to top level.
        assert!(classify_top_level_evaluation(flag, true, true, true));
        // The jump was an ordinary error: R never looked at the flag.
        assert!(!classify_top_level_evaluation(flag, true, false, true));
        // R consumed the flag but the code caught the interrupt and went on.
        assert!(!classify_top_level_evaluation(flag, true, true, false));
        // The request arrived too late to matter.
        assert!(!classify_top_level_evaluation(flag, true, false, false));
        // A jump without any interrupt request.
        assert!(!classify_top_level_evaluation(flag, false, false, true));

        // A request after the evaluation finished is rejected and leaves the
        // flag alone.
        assert_eq!(super::request_ipc_interrupt(None), (false, None));
        assert_eq!(unsafe { *flag }, 0);

        super::R_INTERRUPT_FLAG.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn protected_hooks_preserve_visible_and_input_phases() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flag = Box::leak(Box::new(0));
        super::R_INTERRUPT_FLAG.store(flag, Ordering::Release);
        for phase in [
            super::InterruptPhase::TopLevelEvaluation,
            super::InterruptPhase::Input,
        ] {
            super::begin_ipc_execution(9100, true);
            super::set_ipc_execution_phase(phase);
            super::request_ipc_interrupt(Some(9100));
            super::begin_ipc_protected_evaluation();
            super::finish_ipc_protected_evaluation();
            assert_eq!(super::lock_interrupt_state().phase, phase);
            assert_eq!(
                *flag,
                i32::from(phase == super::InterruptPhase::TopLevelEvaluation)
            );
            super::finish_ipc_execution();
        }
        super::R_INTERRUPT_FLAG.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn silent_boundaries_restore_the_outer_phase_after_nested_evaluation() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flag = Box::leak(Box::new(0));
        super::R_INTERRUPT_FLAG.store(flag, Ordering::Release);
        super::begin_ipc_execution(9103, true);
        super::enable_ipc_silent_boundaries();
        super::request_ipc_interrupt(Some(9103));
        super::begin_ipc_protected_evaluation();
        super::begin_ipc_protected_evaluation();
        super::finish_ipc_protected_evaluation();
        assert_eq!(
            super::lock_interrupt_state().phase,
            super::InterruptPhase::ProtectedEvaluation
        );
        assert_eq!(*flag, 1);
        super::finish_ipc_protected_evaluation();
        assert_eq!(
            super::lock_interrupt_state().phase,
            super::InterruptPhase::Input
        );
        assert_eq!(*flag, 0);
        super::finish_ipc_execution();
        super::R_INTERRUPT_FLAG.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn cancelling_deadlines_preserves_a_user_code_interrupt() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flag = Box::leak(Box::new(0));
        super::R_INTERRUPT_FLAG.store(flag, Ordering::Release);
        super::begin_ipc_execution(9104, true);
        super::set_ipc_execution_phase(super::InterruptPhase::TopLevelEvaluation);
        super::request_ipc_interrupt(Some(9104));
        super::cancel_ipc_deadline();
        assert_eq!(*flag, 1);
        super::finish_ipc_execution();
        super::R_INTERRUPT_FLAG.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn handler_deadline_survives_console_input() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flag = Box::leak(Box::new(0));
        super::R_INTERRUPT_FLAG.store(flag, Ordering::Release);
        for during_input in [false, true] {
            super::begin_handler_ipc_execution(9101, true);
            super::begin_handler_protected_execution(9101);
            if !during_input {
                super::request_ipc_deadline(9101);
            }
            let previous = super::begin_read_console_input();
            if during_input {
                super::request_ipc_deadline(9101);
            }
            super::finish_read_console_input(previous, false);
            assert_eq!(*flag, 1);
            super::finish_handler_protected_execution(9101);
            super::finish_ipc_execution();
        }
        super::R_INTERRUPT_FLAG.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn handler_deadline_is_only_written_inside_its_protected_boundary() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flag: *mut c_int = Box::leak(Box::new(0));
        super::R_INTERRUPT_FLAG.store(flag, Ordering::Release);

        super::begin_handler_ipc_execution(9001, true);
        assert!(super::request_ipc_deadline(9001));
        assert_eq!(
            unsafe { *flag },
            0,
            "argument conversion must not see the flag"
        );
        assert!(super::begin_handler_protected_execution(9001));
        assert_eq!(
            unsafe { *flag },
            1,
            "the protected boundary must receive it"
        );
        assert!(super::finish_handler_protected_execution(9001));
        assert_eq!(
            unsafe { *flag },
            0,
            "finishing must clear the flag immediately"
        );
        super::begin_ipc_execution_finishing();
        assert!(!super::finish_ipc_execution());
        assert!(
            !super::request_ipc_deadline(9001),
            "late timer must be harmless"
        );

        super::R_INTERRUPT_FLAG.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn handler_deadline_checks_id_and_phase_without_sleeping() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !super::request_ipc_deadline(9000),
            "idle has no active deadline"
        );
        super::begin_handler_ipc_execution(9002, true);
        assert!(!super::request_ipc_deadline(9001));
        super::set_ipc_execution_phase(super::InterruptPhase::TopLevelEvaluation);
        assert!(super::request_ipc_deadline(9002));
        assert!(!super::finish_handler_protected_execution(9002));
        super::begin_ipc_execution_finishing();
        assert!(!super::finish_ipc_execution());

        super::begin_handler_ipc_execution(9003, true);
        super::begin_handler_protected_execution(9003);
        assert!(super::request_ipc_deadline(9003));
        assert!(super::finish_handler_protected_execution(9003));
        assert!(
            !super::request_ipc_deadline(9003),
            "finishing rejects late timers"
        );
        super::begin_ipc_execution_finishing();
        super::finish_ipc_execution();
    }

    #[test]
    fn handler_deadlines_survive_a_reentrant_execution_without_cross_talk() {
        let _serial = STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flag: *mut c_int = Box::leak(Box::new(0));
        super::R_INTERRUPT_FLAG.store(flag, Ordering::Release);

        super::begin_handler_ipc_execution(9004, true);
        super::begin_handler_ipc_execution(9005, false);
        assert!(super::request_ipc_deadline(9004));
        assert_eq!(
            unsafe { *flag },
            0,
            "an outer deadline cannot write into the inner call"
        );
        super::begin_ipc_execution_finishing();
        assert!(!super::finish_ipc_execution());
        assert!(super::begin_handler_protected_execution(9004));
        assert_eq!(unsafe { *flag }, 1);
        assert!(super::finish_handler_protected_execution(9004));
        super::begin_ipc_execution_finishing();
        super::finish_ipc_execution();

        super::R_INTERRUPT_FLAG.store(std::ptr::null_mut(), Ordering::Release);
    }
}
