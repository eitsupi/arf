use crate::error::RResult;
use std::env;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::Ordering;

use crate::sys::discovery::get_r_home;
use crate::sys::interrupt::{R_DEFERRED_INTERRUPT_FLAG, R_INTERRUPT_FLAG};
use crate::sys::output::{decode_windows_native, r_write_console_ex};
use crate::sys::read_console::r_read_console;

/// Enable virtual terminal processing on Windows.
///
/// This is required for ANSI escape sequences (colors) to work in the Windows console.
/// Without this, escape codes like `\x1b[31m` (red) are printed literally instead
/// of being interpreted as formatting.
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
pub(super) unsafe fn initialize_r_windows(
    lib: &crate::functions::RLibrary,
    r_args: &[&str],
    disable_cstack_check: bool,
) -> RResult<()> {
    use crate::types::{R_FALSE, Rstart, UImode};
    use std::mem::MaybeUninit;

    log::info!("[WINDOWS] initialize_r_windows called (ark pattern)");

    // Enable ANSI escape sequences for colored output
    enable_windows_virtual_terminal();

    // Get R_HOME and user home
    // These must be set before R_SetParams because it accesses them
    let r_home = get_r_home()?;
    // R converts backslashes in R_HOME to forward slashes, so a canonical
    // Windows verbatim prefix would become the unusable //?/ form. Preserve
    // canonical discovery/loader identity and normalize only this C API input.
    let native_home = windows_r_home_path(&r_home);
    if native_home != r_home {
        let validate = |path: &std::path::Path| {
            path.canonicalize().map_err(|_| {
                crate::error::RError::LibraryNotFound(
                    "Could not validate R_HOME for Windows initialization".into(),
                )
            })
        };
        if validate(&native_home)? != validate(&r_home)? {
            return Err(crate::error::RError::LibraryNotFound(
                "Windows initialization path does not match selected R_HOME".into(),
            ));
        }
    }
    let r_home_cstr = CString::new(native_home.to_string_lossy().as_ref())
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

        // Step 1: Call cmdlineoptions before creating Rstart params (ark pattern).
        // This is where R reads Renviron and applies environment-based memory
        // defaults. Forward only flags that suppress Renviron so that the
        // later R_common_command_line call remains the sole parser for the
        // complete argument list.
        let mut early_args = vec![CString::new("arf").unwrap()];
        for arg in r_args.iter().take_while(|&&arg| arg != "--args") {
            if matches!(*arg, "--no-environ" | "--vanilla") {
                early_args.push(CString::new(*arg).unwrap());
            }
        }
        let mut early_arg_ptrs: Vec<*mut c_char> = early_args
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect();
        (lib.cmdlineoptions)(early_arg_ptrs.len() as c_int, early_arg_ptrs.as_mut_ptr());
        log::info!(
            "[WINDOWS] cmdlineoptions called with {} environment flags",
            early_args.len() - 1
        );

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

        // cmdlineoptions already owns Renviron processing. Keep the parsed
        // Rstart version bits while marking Renviron as handled so R_SetParams
        // does not read the same files a second time. CLI options and memory
        // fields populated above are unaffected by this bitfield update.
        (*params_ptr).bitfield = ((*params_ptr).bitfield & !0xffff) | 1;

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

        // Disable stack checking only for legacy tests that need deep recursion.
        if disable_cstack_check && !lib.r_cstacklimit.is_null() {
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

fn windows_r_home_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf, Prefix};

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path.to_path_buf();
    };
    let mut native = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => PathBuf::from(r"\\").join(server).join(share),
        _ => return path.to_path_buf(),
    };
    native.push(components.as_path());
    native
}

#[cfg(test)]
mod windows_path_tests {
    use super::windows_r_home_path;
    use std::path::Path;

    #[test]
    fn r_home_uses_native_drive_and_unc_prefixes() {
        for (input, expected) in [
            (r"\\?\D:\pixi\lib\R", r"D:\pixi\lib\R"),
            (r"\\?\UNC\server\share\lib\R", r"\\server\share\lib\R"),
            (r"D:\pixi\lib\R", r"D:\pixi\lib\R"),
            (r"\\server\share\lib\R", r"\\server\share\lib\R"),
        ] {
            assert_eq!(windows_r_home_path(Path::new(input)), Path::new(expected));
        }
    }
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
extern "C" fn r_callback() {
    // Do nothing
}

/// Windows callback for ShowMessage.
extern "C" fn r_show_message(msg: *const c_char) {
    if !msg.is_null()
        && let Ok(s) = unsafe { std::ffi::CStr::from_ptr(msg) }.to_str()
    {
        log::info!("[R ShowMessage] {}", s);
    }
}

/// Windows callback for YesNoCancel.
/// Returns 1 for Yes, -1 for No, 0 for Cancel.
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
extern "C" fn r_busy(_which: c_int) {
    // Do nothing - we don't have a busy indicator
}

/// Windows callback for Suicide (fatal error).
extern "C" fn r_suicide(msg: *const c_char) {
    if !msg.is_null()
        && let Ok(s) = unsafe { std::ffi::CStr::from_ptr(msg) }.to_str()
    {
        log::error!("[R FATAL] {}", s);
        eprintln!("R fatal error: {}", s);
    }
    std::process::exit(1);
}
