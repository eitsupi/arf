//! R source resolution and script execution setup.

mod overrides;
mod pixi;
mod r_source;
mod reexec;
mod rig;
mod rig_project;
mod script;

pub(crate) use overrides::override_notice;
pub(crate) use pixi::PixiFailure;
pub(crate) use r_source::{
    RSourceDiagnostic, RSourceOverrideState, RSourceResolutionReport, RSourceSelectionRoute,
    resolve_path_r_home_for_report, resolve_r_source, setup_r,
};
pub(crate) use reexec::capture_loader_target;
#[cfg(unix)]
pub(crate) use reexec::{clear_loader_target, forward_loader_target};
pub(crate) use rig_project::RigProjectFailure;
pub(crate) use script::run_script;
