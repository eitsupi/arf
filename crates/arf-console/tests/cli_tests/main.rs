use std::io::Write;
use std::process::{Output, Stdio};
use tempfile::NamedTempFile;

#[path = "../common/mod.rs"]
mod common;

use common::sanitized_arf_command;

mod cli_surface;
mod config_check;
mod config_warnings;
mod eval;
#[cfg(target_os = "linux")]
mod history_location;
mod ipc_arguments;
mod scripts;
mod top_level_options;
