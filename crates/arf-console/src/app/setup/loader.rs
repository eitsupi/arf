//! Prepare the platform loader search path for the selected R library.

use super::r_source::RSourceResolutionReport;

/// Prepare the loader path for the R library directory of `resolution`.
///
/// This may re-exec the current process. On Unix the selected environment
/// target is forwarded across the re-exec and cleared again once the call
/// returns, so it never lingers when no re-exec happened.
#[cfg_attr(not(unix), allow(unused_variables))]
pub(crate) fn prepare_loader_path(resolution: &RSourceResolutionReport) {
    #[cfg(unix)]
    let loader_result = arf_libr::ensure_ld_library_path_with_pre_exec(|| {
        super::reexec::forward_loader_target(resolution)
    });
    #[cfg(not(unix))]
    let loader_result = arf_libr::ensure_ld_library_path();
    if let Err(e) = loader_result {
        log::warn!("Could not prepare the R library search path: {}", e);
    }
    #[cfg(unix)]
    super::reexec::clear_loader_target();
}

/// Like [`prepare_loader_path`] with explicit re-exec arguments and an extra
/// pre-exec hook, returning the failure to the caller instead of logging it.
#[cfg(unix)]
pub(crate) fn prepare_loader_path_with_args(
    resolution: &RSourceResolutionReport,
    args: &[std::ffi::OsString],
    pre_exec: impl FnOnce(),
) -> arf_libr::RResult<bool> {
    let result = arf_libr::ensure_ld_library_path_with_pre_exec_and_args(args, || {
        super::reexec::forward_loader_target(resolution);
        pre_exec();
    });
    super::reexec::clear_loader_target();
    result
}
