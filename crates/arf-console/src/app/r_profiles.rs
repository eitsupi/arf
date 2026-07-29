//! R profile sourcing after R initialization.

#[cfg(windows)]
use std::path::PathBuf;

/// Source R profile files after R initialization.
///
/// This handles loading of:
/// - Site-level Rprofile.site (unless --no-site-file or --vanilla)
/// - User-level .Rprofile (unless --no-init-file or --vanilla)
///
/// On Windows, R's built-in profile loading is disabled during initialization
/// for compatibility with `globalCallingHandlers()`, so we must manually
/// source these files here.
#[cfg(windows)]
pub(crate) fn source_r_profiles(r_args: &[String]) {
    // Fix .Platform$GUI before any R profiles or packages are loaded.
    // See: https://github.com/eitsupi/arf/issues/168
    arf_harp::override_platform_gui();

    // Get R_HOME from environment (set earlier in setup_r)
    let r_home = match std::env::var("R_HOME") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            log::warn!("R_HOME not set, skipping R profile sourcing");
            return;
        }
    };

    // Source site-level R profile unless --no-site-file or --vanilla
    if !arf_harp::should_ignore_site_r_profile(r_args) {
        arf_harp::source_site_r_profile(&r_home);
    } else {
        log::trace!("Skipping site R profile (--no-site-file or --vanilla)");
    }

    // Source user-level R profile unless --no-init-file or --vanilla
    if !arf_harp::should_ignore_user_r_profile(r_args) {
        arf_harp::source_user_r_profile();
    } else {
        log::trace!("Skipping user R profile (--no-init-file or --vanilla)");
    }

    // Call .First() then .First.sys() to match R's documented startup sequence
    // (see `?Startup`). After profiles are loaded:
    //   1. .First()     — user hook defined in .Rprofile (e.g. vscode-R session watcher)
    //   2. .First.sys() — base package hook that loads default packages (utils, grDevices, ...)
    // On Windows we source profiles manually (profiles disabled in setup_Rmainloop for
    // globalCallingHandlers compatibility), so we must call these hooks manually too.
    arf_harp::call_dot_first();
    arf_harp::call_dot_first_sys();
}
