//! Display the arf changelog in the built-in Markdown pager.

use super::markdown::render_markdown;
use super::{PagerConfig, PagerContent, run};
use crossterm::terminal;
use ratatui::text::Line;
use std::io;

/// Changelog source embedded at compile time from `CHANGELOG.md`.
const CHANGELOG_SOURCE: &str = include_str!(concat!(env!("OUT_DIR"), "/CHANGELOG.md"));

/// Display the arf changelog in an interactive pager.
pub fn display_changelog() {
    if let Err(e) = display_changelog_inner() {
        log::error!("changelog pager error: {}", e);
    }
}

fn display_changelog_inner() -> io::Result<()> {
    let (cols, _) = terminal::size().unwrap_or((80, 24));
    let width = cols as usize;

    let mut content = ChangelogContent {
        lines: ChangelogContent::render_with_width(width),
        last_width: width,
    };

    let config = PagerConfig {
        title: "Changelog",
        footer_hint: "↑↓/jk scroll  q/Esc exit",
        manage_alternate_screen: true,
    };

    run(&mut content, &config)
}

struct ChangelogContent {
    lines: Vec<Line<'static>>,
    last_width: usize,
}

impl ChangelogContent {
    fn render_with_width(width: usize) -> Vec<Line<'static>> {
        render_markdown(CHANGELOG_SOURCE, None, Some(width))
    }
}

impl PagerContent for ChangelogContent {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
        self.lines.get(index).cloned().unwrap_or_default()
    }

    fn on_resize(&mut self, width: usize, _height: usize) -> bool {
        if width != self.last_width {
            self.lines = ChangelogContent::render_with_width(width);
            self.last_width = width;
            true
        } else {
            false
        }
    }
}
