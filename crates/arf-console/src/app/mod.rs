//! Application-level orchestration: config loading, headless mode, CLI
//! subcommand handlers, and R/session setup.

pub(crate) mod commands;
pub(crate) mod config_load;
pub(crate) mod headless;
pub(crate) mod r_home;
pub(crate) mod setup;
