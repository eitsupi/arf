//! Application-level orchestration: config loading, headless mode, CLI
//! subcommand handlers, and R/session setup.

pub(crate) mod commands;
pub(crate) mod config_load;
pub(crate) mod headless;
pub(crate) mod r_profiles;
pub(crate) mod resolve;
pub(crate) mod session_id;
pub(crate) mod setup;
