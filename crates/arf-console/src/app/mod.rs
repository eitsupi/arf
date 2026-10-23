//! Application-level orchestration: config loading, headless mode, CLI
//! subcommand handlers, and R/session setup.

pub(crate) mod arguments;
pub(crate) mod commands;
pub(crate) mod config_load;
pub(crate) mod embedding_api;
pub(crate) mod explicit_r_source;
pub(crate) mod headless;
pub(crate) mod option_scope;
pub(crate) mod r_profiles;
pub(crate) mod r_source_selection;
pub(crate) mod resolve;
pub(crate) mod session_id;
pub(crate) mod setup;
pub(crate) mod startup_env;
