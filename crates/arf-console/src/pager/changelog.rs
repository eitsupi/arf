//! Display the arf changelog in the built-in Markdown pager.

use super::lines_content::{LineStyler, LinesContent};
use super::markdown::render_markdown;
use super::{PagerConfig, run};
use crossterm::terminal;
use ratatui::text::Line;
use std::io;

/// Changelog source embedded at compile time from `CHANGELOG.md`.
const CHANGELOG_SOURCE: &str = include_str!(concat!(env!("OUT_DIR"), "/CHANGELOG.md"));

/// Display the arf changelog in an interactive pager.
pub fn display_changelog() {
    if let Err(e) = display_changelog_inner() {
        log::error!("changelog pager error: {}", e);
        eprintln!("Error: failed to open changelog pager: {}", e);
    }
}

fn display_changelog_inner() -> io::Result<()> {
    let (cols, _) = terminal::size().unwrap_or((80, 24));
    let width = cols as usize;

    let mut content = LinesContent::new(
        ChangelogStyler::render_with_width(width),
        ChangelogStyler { last_width: width },
        None,
    );

    let config = PagerConfig {
        title: "Changelog",
        footer_hint: "↑↓/jk scroll  q/Esc exit",
        manage_alternate_screen: true,
    };

    run(&mut content, &config)
}

/// Re-renders the Markdown changelog when the terminal width changes.
struct ChangelogStyler {
    last_width: usize,
}

impl ChangelogStyler {
    fn render_with_width(width: usize) -> Vec<Line<'static>> {
        render_markdown(CHANGELOG_SOURCE, None, Some(width))
    }
}

impl LineStyler for ChangelogStyler {
    type Line = Line<'static>;

    fn style_line(&self, lines: &[Line<'static>], index: usize) -> Line<'static> {
        lines.get(index).cloned().unwrap_or_default()
    }

    fn on_resize(&mut self, lines: &mut Vec<Line<'static>>, width: usize, _height: usize) {
        if width != self.last_width {
            *lines = Self::render_with_width(width);
            self.last_width = width;
        }
    }
}
