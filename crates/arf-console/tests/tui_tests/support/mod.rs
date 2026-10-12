//! Isolated real arf sessions; terminal transport and emulation belong to tui-test.

mod builder;
mod fixtures;
mod runner;
mod selftests;
mod terminal;

// Binaries which include this module without the whole suite use only part of
// the API.
#[allow(unused_imports)]
pub use builder::{DEFAULT_CONFIG, SharedResources, TerminalBuilder};
#[allow(unused_imports)]
pub use fixtures::{build_formatter_fixture_path, build_rig_project_fixture};
#[allow(unused_imports)]
pub use runner::{run_case, run_case_with};
#[allow(unused_imports)]
pub use terminal::{CliSession, IpcCommand, IpcOutcome, Terminal, wait_for_path_absent};

use std::time::Duration;

pub const WAIT: Duration = Duration::from_secs(30);
pub const POLL_INTERVAL: Duration = Duration::from_millis(25);
pub const PROMPT: &str = "ARF>";
pub const ERROR_PROMPT: &str = "ERR ARF>";
