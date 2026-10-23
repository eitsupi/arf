//! Making arf's native routines available to R code.

/// Install the event publisher and register arf's `.Call` routines with R.
///
/// Call this right after R is initialized, on every startup path. On Windows,
/// where startup profiles are sourced by arf after initialization, call it
/// before they are sourced. On Unix, R runs the profiles during
/// initialization, so the routines do not exist yet while they run.
///
/// A registration failure is logged; R sessions work without the routines.
pub(crate) fn register_embedding_api() {
    // The publisher is in place before any R code can call the routine.
    crate::ipc::install_r_event_publisher();
    if let Err(error) = arf_harp::register_embedding_routines() {
        log::warn!("Could not register arf's native routines with R: {error}");
    }
}
