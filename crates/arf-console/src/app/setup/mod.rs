//! R source resolution and script execution setup.

mod overrides;
mod r_source;
mod rig;
mod script;

pub(crate) use r_source::{
    RSourceDiagnostic, RSourceOverrideState, RSourceResolutionReport,
    resolve_path_r_home_for_report, resolve_r_source, setup_r,
};
pub(crate) use script::run_script;
