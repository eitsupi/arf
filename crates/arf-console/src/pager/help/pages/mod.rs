//! Prepared help page viewing and page selection helpers.

use arf_harp::help_bridge::{PreparedHelpPage, PreparedHelpRequest};
use arf_harp::lib_paths::cached_lib_paths;
use crossterm::terminal;
use std::io;

/// Display help content (Markdown) in an interactive pager.
///
/// Content is rendered from Markdown to styled ratatui lines using
/// `pulldown-cmark`. Both help topics (via `rd2qmd`) and vignettes
/// (via `r-vignette-to-md`) produce Markdown, so this is the unified
/// rendering path.
///
/// Plain text (demo messages, error messages) also renders fine since
/// it contains no Markdown syntax.
pub(super) fn display_help_pager(
    title: &str,
    content: &str,
    manage_alternate_screen: bool,
) -> io::Result<()> {
    use crate::pager::help_content::HelpContent;
    use crate::pager::{PagerConfig, run};

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut content = HelpContent::new(content, cols as usize, rows as usize);

    let config = PagerConfig {
        title,
        footer_hint: "↑↓/jk scroll  / search  q/Esc back",
        manage_alternate_screen,
    };

    run(&mut content, &config)
}

/// Display a prepared help request without rereading the compiled help database.
///
/// Multiple pages require an explicit selector confirmation. Exiting the
/// selector without confirming is a normal cancellation and displays nothing.
pub(crate) fn display_prepared_help_request(request: &PreparedHelpRequest) -> io::Result<()> {
    if request.pages.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "prepared help request has no pages",
        ));
    }
    // R selected and prepared these exact pages already. The cached snapshot
    // only controls additional cross-package navigation and may be empty.
    display_help_pages(request.pages.clone(), cached_lib_paths(), true)
}

pub(super) fn display_help_pages(
    pages: Vec<PreparedHelpPage>,
    libraries: Vec<String>,
    manage_alternate_screen: bool,
) -> io::Result<()> {
    use crate::pager::help_session::HelpViewer;
    use crate::pager::{PagerConfig, run};

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut viewer = HelpViewer::new(pages, libraries, cols as usize, rows as usize);
    let config = PagerConfig {
        manage_alternate_screen,
        ..PagerConfig::default()
    };
    run(&mut viewer, &config)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pager) struct HelpPageSelectorState {
    page_count: usize,
    pub(in crate::pager) selected: Option<usize>,
    confirmed: Option<usize>,
}

impl HelpPageSelectorState {
    pub(in crate::pager) fn new(page_count: usize) -> Self {
        Self {
            page_count,
            selected: None,
            confirmed: None,
        }
    }

    pub(in crate::pager) fn move_up(&mut self) -> bool {
        let Some(selected) = self.selected else {
            self.selected = self.page_count.checked_sub(1);
            return self.selected.is_some();
        };
        if selected == 0 {
            self.selected = None;
            return true;
        }
        self.selected = Some(selected - 1);
        true
    }

    /// Returns `Some(true)` when this selects the first row from the empty
    /// state; the pager must not scroll its viewport for that movement.
    pub(in crate::pager) fn move_down(&mut self) -> Option<bool> {
        match self.selected {
            None if self.page_count > 0 => {
                self.selected = Some(0);
                Some(true)
            }
            Some(selected) if selected + 1 < self.page_count => {
                self.selected = Some(selected + 1);
                Some(false)
            }
            _ => None,
        }
    }

    pub(in crate::pager) fn confirm(&mut self) -> Option<usize> {
        let selected = self.selected?;
        self.confirmed = Some(selected);
        Some(selected)
    }
}

pub(in crate::pager) fn help_page_title(package: &str, display_topic: &str) -> String {
    format!("{package}::{display_topic}")
}

pub(super) fn help_page_load_error_message(error: &io::Error) -> String {
    format!("Unable to load this help topic.\n\n{error}")
}

#[cfg(test)]
mod tests;
