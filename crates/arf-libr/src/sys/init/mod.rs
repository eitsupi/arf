use crate::error::RResult;
use crate::functions::{init_r_library, r_library};
use std::env;

use super::discovery::{
    find_r_library, get_r_home, r_home_from_library_path, r_library_path,
    set_r_path_vars_from_wrapper,
};
#[cfg(unix)]
use super::read_console::r_read_console;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

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
    // SAFETY: This function has the same initialization contract as its caller.
    unsafe { initialize_r_with_args_impl(r_args, false) }
}

/// Initialize R with an unbounded C-stack policy for tests.
///
/// Use this only from an isolated test process. An unbounded C-stack policy
/// can let runaway recursion exhaust the process stack. This is public only so
/// downstream test crates can use it.
///
/// # Safety
/// This function initializes R's global state and must only be called once.
#[doc(hidden)]
pub unsafe fn initialize_r_for_tests() -> RResult<()> {
    // Use default arguments
    // Note: --interactive is only needed on Unix; Windows uses Rstart.r_interactive
    #[cfg(unix)]
    let args = &["--quiet", "--no-save", "--no-restore-data", "--interactive"];
    #[cfg(windows)]
    let args = &["--quiet", "--no-save", "--no-restore-data"];

    // SAFETY: This function has the same initialization contract as its caller.
    unsafe { initialize_r_with_args_for_tests(args) }
}

/// Initialize R with custom arguments and an unbounded C-stack policy for tests.
///
/// Use this only from an isolated test process. An unbounded C-stack policy
/// can let runaway recursion exhaust the process stack.
///
/// # Safety
/// This function initializes R's global state and must only be called once.
unsafe fn initialize_r_with_args_for_tests(r_args: &[&str]) -> RResult<()> {
    // SAFETY: This function has the same initialization contract as its caller.
    unsafe { initialize_r_with_args_impl(r_args, true) }
}

unsafe fn initialize_r_with_args_impl(r_args: &[&str], disable_cstack_check: bool) -> RResult<()> {
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
        unix::initialize_r_unix(lib, r_args, disable_cstack_check)?;
    }

    #[cfg(windows)]
    unsafe {
        windows::initialize_r_windows(lib, r_args, disable_cstack_check)?;
    }

    Ok(())
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
