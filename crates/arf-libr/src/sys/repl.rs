//! POD outcomes and installation for the C-owned interactive R boundary.
//!
//! R may longjmp from parsing, evaluation, or printing. The C trampoline and
//! command driver own every frame crossed by that transfer. Rust callbacks are
//! called only before evaluation starts and at the next native prompt.
//!
//! The installer currently attaches the trampoline through Unix's mutable
//! `ptr_R_ReadConsole` export. Windows selects the C trampoline through
//! `Rstart` before initialization, forwarding to the legacy Rust callback
//! until the driver and its callbacks are installed after initialization.

#[cfg(windows)]
use super::r_read_console;
use crate::{ReadConsoleFunc, SEXP, r_library};
use std::ffi::{CStr, c_char};
use std::os::raw::c_int;
use std::sync::OnceLock;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplFact {
    Unobserved = 0,
    Completed = 1,
    AbortedParse = 2,
    AbortedEval = 3,
    AbortedPrint = 4,
}

/// Whether the native trampoline is reading an outer command or input for R
/// code that is already running.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplInputMode {
    TopLevel = 0,
    Nested = 1,
    /// More source for the active command after its next expression was
    /// incomplete. The command ID and history lifecycle stay unchanged.
    Continuation = 2,
}

/// Result returned by a REPL input callback.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplInputResult {
    Eof = 0,
    Text = 1,
    Cancelled = 2,
}

/// R frame classification returned by the guarded native prompt probe.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplPromptClass {
    Unobserved = 0,
    TopLevel = 1,
    Nested = 2,
}

unsafe extern "C" {
    fn arf_repl_driver_classify_prompt() -> c_int;
    fn arf_repl_driver_prompt_info(
        api: *const NativeApi,
        raw_prompt: *const c_char,
        is_continuation: *mut c_int,
        options_are_ambiguous: *mut c_int,
    ) -> c_int;
}

/// Determine whether R is at its outer evaluation frame. The R call is made
/// entirely inside `R_ToplevelExec` from C, so an R error cannot unwind across
/// this Rust frame. Returns `Unobserved` if the probe fails.
pub fn classify_repl_prompt() -> ReplPromptClass {
    match unsafe { arf_repl_driver_classify_prompt() } {
        1 => ReplPromptClass::TopLevel,
        2 => ReplPromptClass::Nested,
        _ => ReplPromptClass::Unobserved,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplOutcome {
    pub command_id: u64,
    pub expression_id: u32,
    pub fact: ReplFact,
}

impl ReplOutcome {
    pub fn from_native(command_id: u64, expression_id: u32, fact: u8) -> Option<Self> {
        let fact = match fact {
            0 => ReplFact::Unobserved,
            1 => ReplFact::Completed,
            2 => ReplFact::AbortedParse,
            3 => ReplFact::AbortedEval,
            4 => ReplFact::AbortedPrint,
            _ => return None,
        };
        Some(Self {
            command_id,
            expression_id,
            fact,
        })
    }
}

/// Rust input callback invoked by the native C ReadConsole trampoline. For
/// top-level input, allocate and fill `full_source` with
/// [`copy_repl_source`], then set `command_id`; `buffer` is ignored. For nested
/// input, fill R's bounded `buffer` and leave `full_source` null. For
/// continuation input, fill `full_source` with only the new source fragment;
/// C retains the current command ID and lifecycle. Its `prompt` is a stable
/// copy of the current `options('continue')` string. Return `Text`, `Eof`, or
/// `Cancelled`; empty text is not a completed command. The callback has fully
/// returned before R parsing/evaluation begins.
pub type ReplInputCallback = unsafe extern "C" fn(
    mode: c_int,
    prompt: *const c_char,
    buffer: *mut c_char,
    buffer_len: c_int,
    history: c_int,
    full_source: *mut *mut c_char,
    command_id: *mut u64,
    context: *mut std::ffi::c_void,
) -> c_int;

unsafe extern "C" {
    fn arf_repl_driver_alloc_source(length: usize) -> *mut c_char;
}

/// Copy a complete top-level command into C-owned memory for the native
/// trampoline. The C driver frees the allocation after evaluation or recovery.
/// Interior NUL bytes cannot be represented in an R parser input string.
pub fn copy_repl_source(source: &str) -> Option<*mut c_char> {
    if source.as_bytes().contains(&0) {
        return None;
    }
    let ptr = unsafe { arf_repl_driver_alloc_source(source.len()) };
    if ptr.is_null() {
        return None;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(source.as_ptr(), ptr.cast::<u8>(), source.len());
        ptr.add(source.len()).write(0);
    }
    Some(ptr)
}

/// Rust callback invoked once at the next native ReadConsole entry after a
/// command completes or aborts. Nested prompts do not observe a pending fact.
pub type ReplOutcomeCallback = unsafe extern "C" fn(
    command_id: u64,
    expression_id: u32,
    fact: u8,
    context: *mut std::ffi::c_void,
);

/// Prompt classifier owned by the frontend. It is called only when R command
/// evaluation is not running and no input callback is on the stack; an armed
/// command awaiting first execution or Unobserved recovery may still be active.
/// Return the integer value of [`ReplPromptClass`]. `Unobserved` makes the C
/// driver fail closed without consuming a pending outcome or starting input.
/// The prompt text alone may be ambiguous when `options(prompt)` and
/// `options(continue)` match, so the C driver never infers top-level status.
pub type ReplTopLevelPromptCallback =
    unsafe extern "C" fn(prompt: *const c_char, context: *mut std::ffi::c_void) -> c_int;

#[repr(C)]
struct NativeApi {
    mk_string: unsafe extern "C" fn(*const c_char) -> SEXP,
    parse_vector: unsafe extern "C" fn(SEXP, c_int, *mut crate::ParseStatus, SEXP) -> SEXP,
    install: unsafe extern "C" fn(*const c_char) -> SEXP,
    find_var: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
    cons: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
    lcons: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
    eval: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
    protect: unsafe extern "C" fn(SEXP) -> SEXP,
    unprotect: unsafe extern "C" fn(c_int),
    length: unsafe extern "C" fn(SEXP) -> c_int,
    vector_elt: unsafe extern "C" fn(SEXP, isize) -> SEXP,
    logical: unsafe extern "C" fn(SEXP) -> *mut c_int,
    integer: unsafe extern "C" fn(SEXP) -> *mut c_int,
    visible_flag: *mut c_int,
    type_of: unsafe extern "C" fn(SEXP) -> c_int,
    get_option1: unsafe extern "C" fn(SEXP) -> SEXP,
    string_elt: unsafe extern "C" fn(SEXP, isize) -> SEXP,
    char_string: unsafe extern "C" fn(SEXP) -> *const c_char,
    print_value: unsafe extern "C" fn(SEXP),
    toplevel_exec: unsafe extern "C" fn(
        Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
        *mut std::ffi::c_void,
    ) -> c_int,
    unwind_protect: unsafe extern "C" fn(
        Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> SEXP>,
        *mut std::ffi::c_void,
        Option<unsafe extern "C" fn(*mut std::ffi::c_void, c_int)>,
        *mut std::ffi::c_void,
        SEXP,
    ) -> SEXP,
    preserve_object: unsafe extern "C" fn(SEXP),
    release_object: unsafe extern "C" fn(SEXP),
    nil_value: SEXP,
    unbound_value: SEXP,
    global_env: SEXP,
    base_env: SEXP,
}

type NativeReadConsole = unsafe extern "C" fn(*const c_char, *mut c_char, c_int, c_int) -> c_int;
unsafe impl Send for NativeApi {}
unsafe impl Sync for NativeApi {}

unsafe extern "C" {
    #[cfg(any(windows, test))]
    fn arf_repl_driver_read_console(
        prompt: *const c_char,
        buffer: *mut c_char,
        length: c_int,
        history: c_int,
    ) -> c_int;
    #[cfg(any(windows, test))]
    fn arf_repl_driver_set_legacy_read_console(callback: ReadConsoleFunc);
    fn arf_repl_driver_install(
        api: *const NativeApi,
        parser_factory: SEXP,
        top_level_prompt: Option<ReplTopLevelPromptCallback>,
        input: Option<ReplInputCallback>,
        outcome: Option<ReplOutcomeCallback>,
        context: *mut std::ffi::c_void,
        destination: *mut Option<NativeReadConsole>,
    ) -> c_int;
}

static NATIVE_API: OnceLock<NativeApi> = OnceLock::new();
static INSTALLED_READ_CONSOLE: OnceLock<ReadConsoleFunc> = OnceLock::new();

fn native_api(lib: &crate::RLibrary) -> &'static NativeApi {
    NATIVE_API.get_or_init(|| unsafe {
        NativeApi {
            mk_string: lib.rf_mkstring,
            parse_vector: lib.r_parsevector,
            install: lib.rf_install,
            find_var: lib.rf_findvar,
            cons: lib.rf_cons,
            lcons: lib.rf_lcons,
            eval: lib.rf_eval,
            protect: lib.rf_protect,
            unprotect: lib.rf_unprotect,
            length: lib.rf_length,
            vector_elt: lib.vector_elt,
            logical: lib.logical,
            integer: lib.integer,
            visible_flag: lib.r_visible,
            type_of: lib.rf_typeof,
            get_option1: lib.rf_get_option1,
            string_elt: lib.string_elt,
            char_string: lib.r_charsxp,
            print_value: lib.rf_printvalue,
            toplevel_exec: lib.r_toplevelexec,
            unwind_protect: lib.r_unwindprotect,
            preserve_object: lib.r_preserve_object,
            release_object: lib.r_release_object,
            nil_value: *lib.r_nilvalue,
            unbound_value: *lib.r_unboundvalue,
            global_env: *lib.r_globalenv,
            base_env: *lib.r_baseenv,
        }
    })
}

pub(super) fn guarded_prompt_info(raw_prompt: &CStr) -> Option<(bool, bool)> {
    let lib = crate::r_library().ok()?;
    let api = native_api(lib);
    let mut is_continuation = 0;
    let mut options_are_ambiguous = 0;
    let succeeded = unsafe {
        arf_repl_driver_prompt_info(
            api,
            raw_prompt.as_ptr(),
            &mut is_continuation,
            &mut options_are_ambiguous,
        )
    };
    (succeeded != 0).then_some((is_continuation != 0, options_are_ambiguous != 0))
}

#[cfg(unix)]
pub(super) fn installed_read_console_callback() -> Option<ReadConsoleFunc> {
    INSTALLED_READ_CONSOLE.get().copied()
}

/// Windows chooses its ReadConsole callback before R initialization. Install
/// the C trampoline there; until `install_repl_driver` is called, it forwards
/// to the existing Rust callback.
#[cfg(windows)]
pub(super) fn windows_read_console_trampoline() -> ReadConsoleFunc {
    unsafe {
        arf_repl_driver_set_legacy_read_console(Some(r_read_console));
    }
    Some(arf_repl_driver_read_console)
}

/// Install the C-owned native ReadConsole trampoline.
///
/// # Safety
/// Must be called on R's main thread after R initialization and before entering
/// `run_Rmainloop`. The callbacks and context must remain valid for the whole
/// R session. The input callback must release all Rust locks/guards before it
/// returns `Text`, use [`copy_repl_source`] for complete top-level input, and
/// leave `full_source` null for nested input. The prompt classifier must distinguish the true outer
/// command prompt from browser, readline, recovery, and continuation prompts;
/// comparing prompt text alone is insufficient when R prompt options overlap.
#[cfg(any(unix, windows))]
pub unsafe fn install_repl_driver(
    parser_factory: SEXP,
    top_level_prompt: ReplTopLevelPromptCallback,
    input: ReplInputCallback,
    outcome: ReplOutcomeCallback,
    context: *mut std::ffi::c_void,
) -> Result<(), crate::RError> {
    let lib = r_library()?;
    if parser_factory.is_null() {
        return Err(crate::RError::EvalError(
            "failed to install native REPL driver".into(),
        ));
    }
    #[cfg(unix)]
    if lib.ptr_r_readconsole.is_null() {
        return Err(crate::RError::FunctionNotFound("ptr_R_ReadConsole".into()));
    }

    let api = native_api(lib);
    let mut native_callback = None;
    let installed = unsafe {
        arf_repl_driver_install(
            api,
            parser_factory,
            Some(top_level_prompt),
            Some(input),
            Some(outcome),
            context,
            &mut native_callback,
        )
    };
    if installed == 0 || native_callback.is_none() {
        return Err(crate::RError::EvalError(
            "failed to install native REPL driver".into(),
        ));
    }
    #[cfg(unix)]
    {
        unsafe { *lib.ptr_r_readconsole = native_callback };
    }
    let _ = INSTALLED_READ_CONSOLE.set(native_callback);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static LEGACY_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn legacy_read_console(
        _prompt: *const c_char,
        buffer: *mut c_char,
        length: c_int,
        _history: c_int,
    ) -> c_int {
        LEGACY_CALLS.fetch_add(1, Ordering::Relaxed);
        if !buffer.is_null() && length > 1 {
            unsafe {
                *buffer = b'x' as c_char;
                *buffer.add(1) = 0;
            }
        }
        17
    }

    #[test]
    fn native_console_trampoline_forwards_to_legacy_callback_before_install() {
        LEGACY_CALLS.store(0, Ordering::Relaxed);
        unsafe {
            arf_repl_driver_set_legacy_read_console(Some(legacy_read_console));
            let prompt = c"> ";
            let mut buffer = [0 as c_char; 8];
            let result = arf_repl_driver_read_console(
                prompt.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len() as c_int,
                1,
            );
            arf_repl_driver_set_legacy_read_console(None);

            assert_eq!(result, 17);
            assert_eq!(buffer[0], b'x' as c_char);
            assert_eq!(buffer[1], 0);
        }
        assert_eq!(LEGACY_CALLS.load(Ordering::Relaxed), 1);
    }
}
