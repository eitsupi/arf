//! Tests for `history::import`, split by topic into sibling modules.

use super::*;

mod arf_history_source;
mod dedup;
mod import_routing;
mod r_history_parsing;
mod radian_history_parsing;
mod unified_arf_history;

pub(super) fn create_test_targets(temp_dir: &tempfile::TempDir) -> ImportTargets {
    let r_path = temp_dir.path().join("r.db");
    let shell_path = temp_dir.path().join("shell.db");
    ImportTargets {
        r_history: SqliteBackedHistory::with_file(r_path, None, None).unwrap(),
        shell_history: SqliteBackedHistory::with_file(shell_path, None, None).unwrap(),
    }
}
