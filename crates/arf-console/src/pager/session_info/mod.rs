//! Session information display.

mod info;
mod render;

use super::lines_content::{CopyAction, LinesContent};
use super::{PagerConfig, run};
use crate::config::{ConfigFileInfo, HistoryLocationSource, RSourceStatus};
use crate::history::HistoryRuntime;
use crate::repl::reprex::ReprexRuntime;
use crate::repl::state::PromptRuntimeConfig;
use info::SessionInfo;
use render::InfoStyler;

/// Display session information for the :info command in a pager.
pub fn display_session_info(
    prompt_config: &PromptRuntimeConfig,
    reprex: &ReprexRuntime,
    config_file_info: &ConfigFileInfo,
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    history_source: HistoryLocationSource,
    r_source_status: &RSourceStatus,
) {
    let lines = generate_info_lines(
        prompt_config,
        reprex,
        config_file_info,
        r_history,
        shell_history,
        history_source,
        r_source_status,
    );

    let mut content = session_info_content(lines);

    let config = PagerConfig {
        title: "Session Info",
        footer_hint: "↑↓/jk scroll │ c copy │ q exit",
        manage_alternate_screen: true,
    };

    if let Err(e) = run(&mut content, &config) {
        eprintln!("Pager error: {}", e);
    }
}

/// Generate the session information as a vector of lines.
fn generate_info_lines(
    prompt_config: &PromptRuntimeConfig,
    reprex: &ReprexRuntime,
    config_file_info: &ConfigFileInfo,
    r_history: &HistoryRuntime,
    shell_history: &HistoryRuntime,
    history_source: HistoryLocationSource,
    r_source_status: &RSourceStatus,
) -> Vec<String> {
    SessionInfo::collect(
        prompt_config,
        reprex,
        config_file_info,
        r_history,
        shell_history,
        history_source,
        r_source_status,
    )
    .to_lines()
}

/// Pager content for the session info; `c` copies all lines as plain text.
type SessionInfoContent = LinesContent<InfoStyler>;

fn session_info_content(lines: Vec<String>) -> SessionInfoContent {
    let copy = CopyAction {
        text: lines.join("\n"),
        copied_label: "Copied session info to clipboard",
    };
    LinesContent::new(lines, InfoStyler, Some(copy))
}

#[cfg(test)]
mod tests;
