//! Rendered help content and page-local literal search state.

use super::markdown::{LinkDisplay, RenderedMarkdown, render_markdown_document};
use super::{PagerAction, PagerContent};
use arf_harp::help::HelpTarget;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchMatch {
    logical_line: usize,
    /// UTF-8 byte offsets in the unwrapped rendered text.
    start: usize,
    end: usize,
}

pub(super) struct HelpContent {
    document: RenderedMarkdown,
    source: String,
    width: usize,
    height: usize,
    scroll_offset: usize,
    query: String,
    input: Option<String>,
    matches: Vec<SearchMatch>,
    current: Option<usize>,
    selected_link: Option<usize>,
    status: Option<String>,
}

impl HelpContent {
    pub(super) fn new(source: &str, width: usize, height: usize) -> Self {
        let document = Self::render_document(source, width);
        Self {
            document,
            source: source.to_owned(),
            width,
            height,
            scroll_offset: 0,
            query: String::new(),
            input: None,
            matches: Vec::new(),
            current: None,
            selected_link: None,
            status: None,
        }
    }

    fn render_document(source: &str, width: usize) -> RenderedMarkdown {
        render_markdown_document(source, Some("r"), Some(width), |destination| {
            if HelpTarget::from_uri(destination).is_some() {
                LinkDisplay::LabelOnly
            } else {
                LinkDisplay::LabelAndDestination
            }
        })
    }

    pub(super) fn is_search_input(&self) -> bool {
        self.input.is_some()
    }

    pub(super) fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub(super) fn selected_target(&self) -> Option<HelpTarget> {
        HelpTarget::from_uri(&self.document.links.get(self.selected_link?)?.destination)
    }

    pub(super) fn has_help_links(&self) -> bool {
        self.document.links.iter().any(|link| {
            !link.fragments.is_empty() && HelpTarget::from_uri(&link.destination).is_some()
        })
    }

    pub(super) fn select_link(&mut self, forward: bool) -> PagerAction {
        let links: Vec<_> = self
            .document
            .links
            .iter()
            .enumerate()
            .filter(|(_, link)| {
                !link.fragments.is_empty() && HelpTarget::from_uri(&link.destination).is_some()
            })
            .map(|(index, _)| index)
            .collect();
        if links.is_empty() {
            return PagerAction::Redraw;
        }
        let index = match self
            .selected_link
            .and_then(|link| links.iter().position(|i| *i == link))
        {
            Some(index) if forward => (index + 1) % links.len(),
            Some(index) => (index + links.len() - 1) % links.len(),
            None if forward => links
                .iter()
                .position(|i| self.document.links[*i].fragments[0].line >= self.scroll_offset)
                .unwrap_or(0),
            None => links
                .iter()
                .rposition(|i| self.document.links[*i].fragments[0].line <= self.scroll_offset)
                .unwrap_or(links.len() - 1),
        };
        self.selected_link = Some(links[index]);
        self.update_status();
        self.reveal_line(self.document.links[links[index]].fragments[0].line)
    }

    fn reveal_line(&self, line: usize) -> PagerAction {
        let visible_rows = self.height.saturating_sub(2).max(1);
        if line < self.scroll_offset {
            PagerAction::ScrollTo(line)
        } else if line >= self.scroll_offset.saturating_add(visible_rows) {
            PagerAction::ScrollTo(line.saturating_sub(visible_rows - 1))
        } else {
            PagerAction::Redraw
        }
    }

    /// Reflow a saved page around the logical text at its previous viewport.
    pub(super) fn restore_viewport(&mut self, width: usize, height: usize) -> usize {
        if (width, height) == (self.width, self.height) {
            return self.scroll_offset;
        }
        let anchor = self
            .document
            .text
            .iter()
            .enumerate()
            .find_map(|(index, text)| {
                text.fragments
                    .iter()
                    .find(|f| f.line >= self.scroll_offset)
                    .map(|f| (index, f.source_range.start))
            });
        let changed_width = width != self.width;
        self.on_resize(width, height);
        if changed_width && let Some((index, start)) = anchor {
            let fragments = &self.document.text[index].fragments;
            if let Some(fragment) = fragments.iter().find(|f| f.source_range.end > start) {
                self.scroll_offset = fragment.line;
            }
        }
        self.scroll_offset
    }

    fn recompute_matches(&mut self) {
        self.matches.clear();
        if !self.query.is_empty() {
            for (logical_line, text) in self.document.text.iter().enumerate() {
                for (start, _) in text.text.match_indices(&self.query) {
                    self.matches.push(SearchMatch {
                        logical_line,
                        start,
                        end: start + self.query.len(),
                    });
                }
            }
        }
        self.current = None;
    }

    fn visual_ranges(&self, m: SearchMatch) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        self.document.text[m.logical_line]
            .fragments
            .iter()
            .filter_map(move |fragment| {
                let start = m.start.max(fragment.source_range.start);
                let end = m.end.min(fragment.source_range.end);
                (start < end).then(|| {
                    (
                        fragment.line,
                        fragment.visual_start + start - fragment.source_range.start,
                        fragment.visual_start + end - fragment.source_range.start,
                    )
                })
            })
    }

    fn match_line(&self, m: SearchMatch) -> usize {
        self.visual_ranges(m).next().map_or_else(
            || {
                // Whitespace removed at a wrap boundary still belongs to the logical text.
                let fragments = &self.document.text[m.logical_line].fragments;
                fragments
                    .iter()
                    .find(|f| f.source_range.end > m.start)
                    .or_else(|| fragments.last())
                    .map_or(0, |f| f.line)
            },
            |(line, _, _)| line,
        )
    }

    fn update_status(&mut self) {
        // Keep the active link action visible even if a long query is clipped.
        let link_hint = if self.selected_link.is_some() {
            "Enter open  "
        } else {
            ""
        };
        self.status = if let Some(input) = &self.input {
            Some(format!("/{input}|  Enter search  Esc cancel"))
        } else if self.query.is_empty() {
            None
        } else if self.matches.is_empty() {
            Some(format!(
                "{link_hint}No matches for: {}  / search  q clear search",
                self.query
            ))
        } else {
            Some(format!(
                "{link_hint}/{} [{}/{}]  n/N next/previous  / search  q clear search",
                self.query,
                self.current.map_or(0, |index| index + 1),
                self.matches.len(),
            ))
        };
    }

    fn move_match(&mut self, forward: bool) -> PagerAction {
        if self.matches.is_empty() {
            self.update_status();
            return PagerAction::Redraw;
        }

        let count = self.matches.len();
        let index = match self.current {
            Some(index) if forward => (index + 1) % count,
            Some(index) => (index + count - 1) % count,
            None if forward => self
                .matches
                .iter()
                .position(|m| self.match_line(*m) >= self.scroll_offset)
                .unwrap_or(0),
            None => self
                .matches
                .iter()
                .rposition(|m| self.match_line(*m) <= self.scroll_offset)
                .unwrap_or(count - 1),
        };
        self.current = Some(index);
        self.selected_link = None;
        self.update_status();

        self.reveal_line(self.match_line(self.matches[index]))
    }
}

impl PagerContent for HelpContent {
    fn line_count(&self) -> usize {
        self.document.lines.len()
    }

    fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
        let mut line = self.document.lines.get(index).cloned().unwrap_or_default();
        if let Some(m) = self.current.map(|current| self.matches[current])
            && let Some((start, end)) = self
                .visual_ranges(m)
                .find_map(|(line, start, end)| (line == index).then_some((start, end)))
        {
            let highlight = Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
            highlight_range(&mut line, start, end, highlight);
        }
        if let Some(link) = self.selected_link.map(|index| &self.document.links[index]) {
            for fragment in link.fragments.iter().filter(|f| f.line == index) {
                highlight_range(
                    &mut line,
                    fragment.byte_range.start,
                    fragment.byte_range.end,
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
        line
    }

    fn prepare_render(&mut self, scroll_offset: usize) {
        self.scroll_offset = scroll_offset;
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<PagerAction> {
        // Keep the pager's emergency exit keys available in input mode.
        if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c' | 'd')) {
            return None;
        }
        if let Some(input) = &mut self.input {
            match code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => {
                    let input = self.input.take().expect("search input mode is active");
                    self.selected_link = None;
                    if !input.is_empty() && input != self.query {
                        self.query = input;
                        self.recompute_matches();
                    }
                    return Some(self.move_match(true));
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(ch)
                    if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    input.push(ch);
                }
                _ => {}
            }
            self.update_status();
            return Some(PagerAction::Redraw);
        }
        match (code, modifiers) {
            (KeyCode::Char('/'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.input = Some(String::new());
                self.update_status();
                Some(PagerAction::Redraw)
            }
            (KeyCode::Char('q'), KeyModifiers::NONE) if !self.query.is_empty() => {
                self.query.clear();
                self.matches.clear();
                self.current = None;
                self.update_status();
                Some(PagerAction::Redraw)
            }
            (KeyCode::Char('n'), KeyModifiers::NONE) if !self.query.is_empty() => {
                Some(self.move_match(true))
            }
            (KeyCode::Char('N'), KeyModifiers::NONE | KeyModifiers::SHIFT)
                if !self.query.is_empty() =>
            {
                Some(self.move_match(false))
            }
            _ => None,
        }
    }

    fn feedback_message(&self) -> Option<&str> {
        self.status.as_deref()
    }

    fn on_resize(&mut self, width: usize, height: usize) {
        self.height = height;
        // A new viewport invalidates the selected visual match, not the query.
        self.current = None;
        if width == self.width {
            self.update_status();
            return;
        }
        self.document = Self::render_document(&self.source, width);
        self.width = width;
        self.recompute_matches();
        self.update_status();
    }
}

fn highlight_range(line: &mut Line<'static>, start: usize, end: usize, highlight: Style) {
    let mut offset = 0;
    let mut spans = Vec::new();
    for span in &line.spans {
        let text = span.content.as_ref();
        let start = start.saturating_sub(offset).min(text.len());
        let end = end.saturating_sub(offset).min(text.len());
        if start < end {
            if start > 0 {
                spans.push(Span::styled(text[..start].to_owned(), span.style));
            }
            spans.push(Span::styled(
                text[start..end].to_owned(),
                span.style.patch(highlight),
            ));
            if end < text.len() {
                spans.push(Span::styled(text[end..].to_owned(), span.style));
            }
        } else {
            spans.push(span.clone());
        }
        offset += text.len();
    }
    line.spans = spans;
}

#[cfg(test)]
mod tests;
