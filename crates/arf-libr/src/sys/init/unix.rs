use crate::error::RResult;
use std::env;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::Ordering;

use crate::sys::interrupt::R_INTERRUPT_FLAG;
use crate::sys::output::r_write_console_ex;

/// Unix-specific R initialization.
pub(super) unsafe fn initialize_r_unix(
    lib: &crate::functions::RLibrary,
    r_args: &[&str],
    disable_cstack_check: bool,
) -> RResult<()> {
    normalize_empty_r_profile_user();

    unsafe {
        // Set R_running_as_main_program before initialization (like ark does)
        if !lib.r_running_as_main_program.is_null() {
            *lib.r_running_as_main_program = 1;
        }

        // Disable R's signal handlers before initialization.
        // With R_SignalHandlers = 0, setup_Rmainloop skips init_signal_handlers()
        // entirely: no SIGINT handler, but also no SIGSEGV/SIGILL/SIGBUS crash
        // handlers or SIGUSR1/SIGUSR2 save-and-quit handlers, which are
        // undesirable for an embedded frontend. The REPL installs its own
        // Ctrl+C handler (see repl/mod.rs) that sets R_interrupts_pending,
        // replicating R's standard handleInterrupt behavior.
        if !lib.r_signalhandlers.is_null() {
            *lib.r_signalhandlers = 0;
        }

        // Store the interrupt flag pointer for use by the Ctrl+C handler
        if !lib.r_interrupts_pending.is_null() {
            R_INTERRUPT_FLAG.store(lib.r_interrupts_pending, Ordering::Release);
        }

        // Prepare arguments for R initialization
        let mut args: Vec<CString> = vec![CString::new("arf").unwrap()];
        for arg in r_args {
            if let Ok(cstr) = CString::new(*arg) {
                args.push(cstr);
            }
        }
        let arg_ptrs: Vec<*const c_char> = args.iter().map(|s| s.as_ptr()).collect();

        // Initialize R
        (lib.rf_initialize_r)(args.len() as c_int, arg_ptrs.as_ptr());

        // Mark R as interactive
        if !lib.r_interactive.is_null() {
            *lib.r_interactive = 1;
        }

        // Disable stack checking only for legacy tests that need deep recursion.
        if disable_cstack_check && !lib.r_cstacklimit.is_null() {
            *lib.r_cstacklimit = usize::MAX;
        }

        // Redirect console output (set file pointers to NULL so callbacks are used)
        if !lib.r_consolefile.is_null() {
            *lib.r_consolefile = std::ptr::null_mut();
        }
        if !lib.r_outputfile.is_null() {
            *lib.r_outputfile = std::ptr::null_mut();
        }

        // Disable default console write
        if !lib.ptr_r_writeconsole.is_null() {
            *lib.ptr_r_writeconsole = None;
        }

        // Set our console write callback
        if !lib.ptr_r_writeconsoleex.is_null() {
            *lib.ptr_r_writeconsoleex = Some(r_write_console_ex);
        }

        // Setup R main loop (but don't run it)
        (lib.setup_rmainloop)();
    }

    Ok(())
}

/// Unset an empty `R_PROFILE_USER` so R uses its normal user-profile search.
///
/// Some R terminal integrations set `R_PROFILE_USER` to a wrapper script on
/// startup, then reset it to `""` after the script runs. An exec-based restart
/// inherits that value, and R treats it as an explicit instruction to skip user
/// profiles instead of falling back to `.Rprofile`.
fn normalize_empty_r_profile_user() {
    if env::var("R_PROFILE_USER").as_deref() == Ok("") {
        // SAFETY: R initialization is single-threaded.
        unsafe { env::remove_var("R_PROFILE_USER") };
    }
}
