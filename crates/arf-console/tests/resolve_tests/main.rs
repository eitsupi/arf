//! `arf r resolve` descriptor contracts, plus startup without a usable R.

#[path = "../common/mod.rs"]
mod common;

mod descriptor;
mod env_precedence;
mod fixtures;
mod normalize;
#[cfg(unix)]
mod project_file;
#[cfg(unix)]
mod rig;
#[cfg(not(windows))]
mod startup_without_r;
