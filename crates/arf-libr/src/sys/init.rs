use crate::error::RResult;
use crate::functions::{init_r_library, r_library};
use std::env;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::Ordering;

use super::discovery::{
    find_r_library, get_r_home, r_home_from_library_path, r_library_path,
    set_r_path_vars_from_wrapper,
};
#[cfg(windows)]
use super::interrupt::R_DEFERRED_INTERRUPT_FLAG;
use super::interrupt::R_INTERRUPT_FLAG;
#[cfg(windows)]
use super::output::decode_windows_native;
use super::output::r_write_console_ex;
use super::r_read_console;

/// Initialize R with default settings.
///
/// # Safety
/// This function initializes R's global state and must only be called once.
pub unsafe fn initialize_r() -> RResult<()> {
    // Use default arguments
    // Note: --interactive is only needed on Unix; Windows uses Rstart.r_interactive
    #[cfg(unix)]
    let args = &["--quiet", "--no-save", "--no-restore-data", "--interactive"];
    #[cfg(windows)]
    let args = &["--quiet", "--no-save", "--no-restore-data"];

    // SAFETY: We're forwarding to initialize_r_with_args which handles the unsafe operations
    unsafe { initialize_r_with_args(args) }
}

/// Initialize R with custom arguments.
///
/// The `r_args` parameter should contain R command-line arguments like
/// `["--quiet", "--no-save", "--no-restore"]`.
///
/// # Safety
/// This function initializes R's global state and must only be called once.
pub unsafe fn initialize_r_with_args(r_args: &[&str]) -> RResult<()> {
    // Enable color output for R packages (cli, crayon, etc.)
    // Embedded R doesn't have a TTY, so we force color output via environment variables.
    // SAFETY: We're in single-threaded initialization before R starts
    unsafe {
        // CLICOLOR_FORCE=1 forces color output even without a TTY
        if env::var("NO_COLOR").is_err() && env::var("CLICOLOR_FORCE").is_err() {
            env::set_var("CLICOLOR_FORCE", "1");
        }
        // COLORTERM indicates color support level
        if env::var("COLORTERM").is_err() {
            env::set_var("COLORTERM", "truecolor");
        }
    }

    // Find and load R library
    let lib_path = find_r_library()?;
    init_r_library(&lib_path)?;

    // R may have found the library through PATH after an invalid R_HOME was
    // inherited from the environment. Make R_HOME match the library we are
    // about to initialize so R can load its system Renviron and base package.
    let r_home_is_valid = env::var_os("R_HOME")
        .is_some_and(|r_home| r_library_path(std::path::Path::new(&r_home)).exists());
    if !r_home_is_valid && let Some(r_home) = r_home_from_library_path(&lib_path) {
        // SAFETY: We're in single-threaded initialization
        unsafe { env::set_var("R_HOME", &r_home) };
    }

    // NOTE: R_LIBS_SITE is intentionally NOT set here. R handles it via
    // Renviron (defaulting to R_HOME/site-library when unset) and .Library
    // (R_HOME/library) is always included regardless. See GitHub issue #86.

    // Set R_DOC_DIR, R_SHARE_DIR, R_INCLUDE_DIR if not already set.
    // These are normally exported by R's shell wrapper script ($R_HOME/bin/R),
    // which is bypassed when embedding R. On distributions where these paths
    // differ from the default $R_HOME/<component> (e.g., Fedora/RHEL), this
    // causes R.home("doc") etc. to return non-existent paths.
    // SAFETY: We're in single-threaded initialization
    if let Ok(r_home) = get_r_home() {
        set_r_path_vars_from_wrapper(&r_home);
    }

    let lib = r_library()?;

    // Platform-specific initialization
    #[cfg(unix)]
    unsafe {
        initialize_r_unix(lib, r_args)?;
    }

    #[cfg(windows)]
    unsafe {
        initialize_r_windows(lib, r_args)?;
    }

    Ok(())
}

/// Unix-specific R initialization.
#[cfg(unix)]
unsafe fn initialize_r_unix(lib: &crate::functions::RLibrary, r_args: &[&str]) -> RResult<()> {
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

        // Disable stack checking (required for embedded R)
        if !lib.r_cstacklimit.is_null() {
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
#[cfg(unix)]
fn normalize_empty_r_profile_user() {
    if env::var("R_PROFILE_USER").as_deref() == Ok("") {
        // SAFETY: R initialization is single-threaded.
        unsafe { env::remove_var("R_PROFILE_USER") };
    }
}

/// Enable virtual terminal processing on Windows.
///
/// This is required for ANSI escape sequences (colors) to work in the Windows console.
/// Without this, escape codes like `\x1b[31m` (red) are printed literally instead
/// of being interpreted as formatting.
#[cfg(windows)]
fn enable_windows_virtual_terminal() {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
    }

    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    unsafe {
        // Enable for stdout
        let stdout = std::io::stdout().as_raw_handle();
        let mut mode: u32 = 0;
        if GetConsoleMode(stdout as *mut _, &mut mode) != 0
            && SetConsoleMode(stdout as *mut _, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        {
            log::debug!("[WINDOWS] Enabled virtual terminal processing for stdout");
        }

        // Enable for stderr
        let stderr = std::io::stderr().as_raw_handle();
        if GetConsoleMode(stderr as *mut _, &mut mode) != 0
            && SetConsoleMode(stderr as *mut _, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        {
            log::debug!("[WINDOWS] Enabled virtual terminal processing for stderr");
        }
    }
}

/// Windows-specific R initialization.
///
/// On Windows, R uses a params-based approach instead of global function pointers.
/// We need to create an Rstart struct, set callbacks on it, then call R_SetParams.
/// This follows the ark pattern for Windows R initialization.
#[cfg(windows)]
unsafe fn initialize_r_windows(lib: &crate::functions::RLibrary, r_args: &[&str]) -> RResult<()> {
    use crate::types::{R_FALSE, Rstart, UImode};
    use std::mem::MaybeUninit;

    log::info!("[WINDOWS] initialize_r_windows called (ark pattern)");

    // Enable ANSI escape sequences for colored output
    enable_windows_virtual_terminal();

    // Get R_HOME and user home
    // These must be set before R_SetParams because it accesses them
    let r_home = get_r_home()?;
    let r_home_cstr = CString::new(r_home.to_string_lossy().as_ref())
        .map_err(|_| crate::error::RError::LibraryNotFound("Invalid R_HOME path".to_string()))?;

    // Use R's getRUser() to determine the user home directory.
    // On Windows, R resolves ~ to the Documents folder (via SHGetKnownFolderPath),
    // NOT USERPROFILE. Using USERPROFILE would cause R_LIBS_USER paths like
    // ~/R/win-library to resolve to the wrong location, especially when the
    // Documents folder has been moved to a different drive.
    // See: https://github.com/eitsupi/arf/issues/65
    let user_home = get_r_user_home(lib);
    let user_home_cstr = CString::new(user_home)
        .map_err(|_| crate::error::RError::LibraryNotFound("Invalid user home path".to_string()))?;

    unsafe {
        // Disable R's signal handlers first.
        // We install our own Ctrl+C handler that sets UserBreak in the REPL.
        if !lib.r_signalhandlers.is_null() {
            *lib.r_signalhandlers = 0;
        }

        // Store the interrupt flag pointer for use by the Ctrl+C handler
        if !lib.user_break.is_null() {
            R_INTERRUPT_FLAG.store(lib.user_break, Ordering::Release);
        }

        // Store the deferred interrupt flag pointer so stale interrupts
        // deferred under R_interrupts_suspended can be cleared too
        if !lib.r_interrupts_pending.is_null() {
            R_DEFERRED_INTERRUPT_FLAG.store(lib.r_interrupts_pending, Ordering::Release);
        }

        // Step 1: Call cmdlineoptions with empty args (ark pattern)
        // R does initialization here that's not accessible in any other way
        let empty_arg = CString::new("arf").unwrap();
        let mut empty_args: Vec<*mut c_char> = vec![empty_arg.as_ptr() as *mut c_char];
        (lib.cmdlineoptions)(1, empty_args.as_mut_ptr());
        log::info!("[WINDOWS] cmdlineoptions called with empty args");

        // Step 2: Create and initialize the Rstart params struct
        let mut params: MaybeUninit<Rstart> = MaybeUninit::uninit();
        let params_ptr = params.as_mut_ptr();

        // Initialize with defaults (version 0 for compatibility)
        (lib.r_defparamsex)(params_ptr, 0);

        // Step 3: Process command line arguments via R_common_command_line (ark pattern)
        // This sets params fields like R_Quiet, R_Verbose, SaveAction, RestoreAction
        let mut args: Vec<CString> = vec![CString::new("arf").unwrap()];
        for arg in r_args {
            if let Ok(cstr) = CString::new(*arg) {
                args.push(cstr);
            }
        }
        let mut arg_ptrs: Vec<*mut c_char> =
            args.iter().map(|s| s.as_ptr() as *mut c_char).collect();
        let mut argc = args.len() as c_int;
        (lib.r_common_command_line)(&mut argc, arg_ptrs.as_mut_ptr(), params_ptr);
        log::info!(
            "[WINDOWS] R_common_command_line processed {} args",
            args.len()
        );

        // Step 4: Configure the params
        (*params_ptr).r_interactive = 1;
        // Use RGui mode during initialization so that R_SetParams correctly
        // sets up console callbacks (ReadConsole, WriteConsoleEx, etc.).
        // After setup_Rmainloop(), we switch to LinkDLL mode to prevent
        // R's do_system() from invalidating standard handles, which causes
        // system()/system2() to hang. This follows the sircon pattern.
        // See: https://github.com/eitsupi/arf/issues/116
        (*params_ptr).character_mode = UImode::RGui;

        // Disable R's built-in profile loading during initialization.
        // We source .Rprofile manually in arf-console/src/main.rs after R is
        // fully initialized. This allows globalCallingHandlers() to work in
        // .Rprofile (used by packages like prompt).
        // See: https://github.com/posit-dev/ark/blob/ca75dbb466875c8d3cd04ad8fbf5684d59b31ba1/crates/ark/src/startup.rs
        (*params_ptr).load_init_file = R_FALSE;
        (*params_ptr).load_site_file = R_FALSE;

        // Set console callbacks (matching ark pattern)
        (*params_ptr).write_console = None;
        (*params_ptr).write_console_ex = Some(r_write_console_ex);
        (*params_ptr).read_console = Some(r_read_console);
        (*params_ptr).show_message = Some(r_show_message);
        (*params_ptr).yes_no_cancel = Some(r_yes_no_cancel);
        (*params_ptr).callback = Some(r_callback);
        (*params_ptr).busy = Some(r_busy);
        (*params_ptr).suicide = Some(r_suicide);
        log::info!(
            "[WINDOWS] Console callbacks set (read_console={:p})",
            r_read_console as *const ()
        );

        // Set paths
        (*params_ptr).rhome = r_home_cstr.as_ptr() as *mut c_char;
        (*params_ptr).home = user_home_cstr.as_ptr() as *mut c_char;

        // Step 5: Apply the params to R's globals
        (lib.r_setparams)(params_ptr);
        log::info!("[WINDOWS] R_SetParams called");

        // Disable stack checking (for testing - embedded R needs this)
        if !lib.r_cstacklimit.is_null() {
            *lib.r_cstacklimit = usize::MAX;
        }

        // Step 6: Initialize graphapp (required for Windows GUI)
        if let Some(ga_initapp) = lib.ga_initapp {
            ga_initapp(0, std::ptr::null_mut());
            log::info!("[WINDOWS] GA_initapp called");
        }

        // Read console config (required for proper console initialization)
        (lib.readconsolecfg)();
        log::info!("[WINDOWS] readconsolecfg called");

        // Step 7: Switch CharacterMode from RGui to LinkDLL.
        //
        // R's do_system() checks CharacterMode and, when it is RGui, calls
        // SetStdHandle(STD_INPUT_HANDLE, INVALID_HANDLE_VALUE) (and similarly
        // for stdout/stderr) before spawning child processes. This invalidates
        // the standard handles and causes system()/system2() to hang.
        //
        // By switching to LinkDLL before setup_Rmainloop(), we keep the
        // callback setup from RGui mode (applied by R_SetParams) while
        // avoiding the handle invalidation in do_system(). This is the same
        // approach used by sircon.
        if !lib.character_mode.is_null() {
            *lib.character_mode = UImode::LinkDLL as c_int;
            log::info!("[WINDOWS] CharacterMode switched from RGui to LinkDLL");
        } else {
            log::error!(
                "[WINDOWS] Could not load CharacterMode symbol from R.dll; system() may hang"
            );
        }

        // Step 8: Setup R main loop (but don't run it yet)
        log::info!("[WINDOWS] Calling setup_Rmainloop...");
        (lib.setup_rmainloop)();
        log::info!("[WINDOWS] setup_Rmainloop completed");
    }

    Ok(())
}

/// Get R's user home directory on Windows using `getRUser()` from R.dll.
///
/// `getRUser()` returns the directory that R uses for `~`, following this
/// search order:
/// 1. `R_USER` environment variable
/// 2. `HOME` environment variable
/// 3. `SHGetKnownFolderPath(FOLDERID_Documents)` (the Documents folder)
/// 4. `HOMEDRIVE` + `HOMEPATH`
/// 5. Current working directory
///
/// This is important because R's `~` resolves to the Documents folder, not
/// `USERPROFILE`. When users move their Documents folder to a different drive,
/// `USERPROFILE` (e.g. `C:\Users\name`) and Documents (e.g. `D:\Users\name\Documents`)
/// diverge, causing `R_LIBS_USER` paths like `~/R/win-library` to fail.
///
/// Falls back to `USERPROFILE` if `getRUser()` returns NULL.
///
/// # Memory safety
/// `getRUser()` stores its result in a static buffer inside R.dll
/// (`UserRHome`), so the returned pointer is valid for the lifetime of the
/// process and does not need to be freed by the caller.
#[cfg(windows)]
fn get_r_user_home(lib: &crate::functions::RLibrary) -> String {
    let r_user = unsafe { (lib.get_r_user)() };

    if !r_user.is_null() {
        let cstr = unsafe { std::ffi::CStr::from_ptr(r_user) };
        let bytes = cstr.to_bytes();

        // getRUser() returns a path encoded in the system's ANSI code page
        // (e.g. CP932/Shift-JIS on Japanese Windows). Try UTF-8 first (which
        // covers ASCII paths), then fall back to proper code page conversion
        // via decode_windows_native() to handle non-ASCII usernames.
        let path = match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => {
                log::debug!(
                    "[WINDOWS] getRUser() returned non-UTF-8 path, decoding from system code page"
                );
                decode_windows_native(bytes).into_owned()
            }
        };

        log::debug!("[WINDOWS] getRUser() returned: {}", path);
        return path;
    }

    log::warn!("[WINDOWS] getRUser() returned NULL, falling back to USERPROFILE");
    env::var("USERPROFILE")
        .or_else(|_| env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string())
}

/// Windows callback for ProcessEvents (no-op).
#[cfg(windows)]
extern "C" fn r_callback() {
    // Do nothing
}

/// Windows callback for ShowMessage.
#[cfg(windows)]
extern "C" fn r_show_message(msg: *const c_char) {
    if !msg.is_null()
        && let Ok(s) = unsafe { std::ffi::CStr::from_ptr(msg) }.to_str()
    {
        log::info!("[R ShowMessage] {}", s);
    }
}

/// Windows callback for YesNoCancel.
/// Returns 1 for Yes, -1 for No, 0 for Cancel.
#[cfg(windows)]
extern "C" fn r_yes_no_cancel(question: *const c_char) -> c_int {
    // This is used during R's CleanUp when SA_SAVEASK is used.
    // We return -1 (No) to avoid saving.
    if !question.is_null()
        && let Ok(s) = unsafe { std::ffi::CStr::from_ptr(question) }.to_str()
    {
        log::warn!("[R YesNoCancel] Ignoring question: '{}'. Returning NO.", s);
    }
    -1 // NO
}

/// Windows callback for Busy indicator.
#[cfg(windows)]
extern "C" fn r_busy(_which: c_int) {
    // Do nothing - we don't have a busy indicator
}

/// Windows callback for Suicide (fatal error).
#[cfg(windows)]
extern "C" fn r_suicide(msg: *const c_char) {
    if !msg.is_null()
        && let Ok(s) = unsafe { std::ffi::CStr::from_ptr(msg) }.to_str()
    {
        log::error!("[R FATAL] {}", s);
        eprintln!("R fatal error: {}", s);
    }
    std::process::exit(1);
}

/// Run R's main event loop.
///
/// This calls R's `run_Rmainloop()` which never returns normally.
/// It will continuously call the ReadConsole callback to get user input.
///
/// # Safety
/// R must be initialized before calling this function.
pub unsafe fn run_r_mainloop() {
    log::info!("run_r_mainloop: entering");

    let lib = match r_library() {
        Ok(lib) => lib,
        Err(e) => {
            log::error!("run_r_mainloop: failed to get r_library: {:?}", e);
            return;
        }
    };

    // Set up our ReadConsole callback (Unix only - Windows sets it in R_SetParams)
    #[cfg(unix)]
    unsafe {
        if !lib.ptr_r_readconsole.is_null() {
            *lib.ptr_r_readconsole = Some(r_read_console);
        }
    }

    // Check R_Interactive value before running mainloop
    unsafe {
        if !lib.r_interactive.is_null() {
            log::info!("run_r_mainloop: R_Interactive = {}", *lib.r_interactive);
        }
    }

    log::info!("run_r_mainloop: calling run_Rmainloop");

    // Run R's main loop - this doesn't return
    unsafe {
        (lib.run_rmainloop)();
    }

    log::info!("run_r_mainloop: run_Rmainloop returned (unexpected)");
}
