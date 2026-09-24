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
use std::ffi::c_char;
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

/// Rust input callback invoked by the native C ReadConsole trampoline. It must
/// fill `buffer` and set `command_id` when returning positive; the callback has
/// fully returned before R parsing/evaluation begins.
pub type ReplInputCallback = unsafe extern "C" fn(
    prompt: *const c_char,
    buffer: *mut c_char,
    buffer_len: c_int,
    history: c_int,
    command_id: *mut u64,
    context: *mut std::ffi::c_void,
) -> c_int;

/// Rust callback invoked once at the next native ReadConsole entry after a
/// command completes or aborts. Nested prompts do not observe a pending fact.
pub type ReplOutcomeCallback = unsafe extern "C" fn(
    command_id: u64,
    expression_id: u32,
    fact: u8,
    context: *mut std::ffi::c_void,
);

/// Prompt classifier owned by the frontend. The prompt text alone may be
/// ambiguous when `options(prompt)` and `options(continue)` match, so the C
/// driver never infers top-level status itself.
pub type ReplTopLevelPromptCallback =
    unsafe extern "C" fn(prompt: *const c_char, context: *mut std::ffi::c_void) -> c_int;

#[repr(C)]
struct NativeApi {
    mk_string: unsafe extern "C" fn(*const c_char) -> SEXP,
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

pub(super) fn installed_read_console_callback() -> Option<ReadConsoleFunc> {
    INSTALLED_READ_CONSOLE.get().copied()
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
/// returns positive. The prompt classifier must distinguish the true outer
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
    let api = NATIVE_API.get_or_init(|| unsafe {
        NativeApi {
            mk_string: lib.rf_mkstring,
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
    });
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
        if lib.ptr_r_readconsole.is_null() {
            return Err(crate::RError::FunctionNotFound("ptr_R_ReadConsole".into()));
        }
        unsafe { *lib.ptr_r_readconsole = native_callback };
    }
    let _ = INSTALLED_READ_CONSOLE.set(native_callback);
    Ok(())
}
