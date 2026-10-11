//! R source resolution and script execution setup.

mod overrides;
mod pixi;
mod r_source;
mod rig;
mod script;

pub(crate) use pixi::reexec::capture_loader_target;
#[cfg(unix)]
pub(crate) use pixi::reexec::{clear_loader_target, forward_loader_target};
pub(crate) use r_source::{
    RSourceDiagnostic, RSourceOverrideState, RSourceResolutionReport, RSourceSelectionRoute,
    resolve_path_r_home_for_report, resolve_r_source, setup_r,
};
pub(crate) use script::run_script;
