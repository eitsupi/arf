//! Rendered help content and page-local literal search state.

use super::markdown::render_markdown;
use super::{PagerAction, PagerContent};
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchMatch {
    line: usize,
    /// UTF-8 byte offsets in the concatenated rendered spans of this line.
    start: usize,
    end: usize,
}

pub(super) struct HelpContent {
    lines: Vec<Line<'static>>,
    source: String,
    width: usize,
    height: usize,
    scroll_offset: usize,
    query: String,
    input: Option<String>,
    matches: Vec<SearchMatch>,
    current: Option<usize>,
    status: Option<String>,
}

impl HelpContent {
    pub(super) fn new(source: &str, width: usize, height: usize) -> Self {
        Self {
            lines: render_markdown(source, Some("r"), Some(width)),
            source: source.to_owned(),
            width,
            height,
            scroll_offset: 0,
            query: String::new(),
            input: None,
            matches: Vec::new(),
            current: None,
            status: None,
        }
    }

    fn recompute_matches(&mut self) {
        self.matches.clear();
        if !self.query.is_empty() {
            for (line, text) in self.lines.iter().enumerate() {
                for (start, _) in text.to_string().match_indices(&self.query) {
                    self.matches.push(SearchMatch {
                        line,
                        start,
                        end: start + self.query.len(),
                    });
                }
            }
        }
        self.current = None;
    }

    fn update_status(&mut self) {
        self.status = if let Some(input) = &self.input {
            Some(format!("/{input}|  Enter search  Esc cancel"))
        } else if self.query.is_empty() {
            None
        } else if self.matches.is_empty() {
            Some(format!(
                "No matches for: {}  / search  q clear search",
                self.query
            ))
        } else {
            Some(format!(
                "/{} [{}/{}]  n/N next/previous  / search  q clear search",
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
                .position(|m| m.line >= self.scroll_offset)
                .unwrap_or(0),
            None => self
                .matches
                .iter()
                .rposition(|m| m.line <= self.scroll_offset)
                .unwrap_or(count - 1),
        };
        self.current = Some(index);
        self.update_status();

        let line = self.matches[index].line;
        let visible_rows = self.height.saturating_sub(2).max(1);
        if line < self.scroll_offset {
            PagerAction::ScrollTo(line)
        } else if line >= self.scroll_offset.saturating_add(visible_rows) {
            PagerAction::ScrollTo(line.saturating_sub(visible_rows - 1))
        } else {
            PagerAction::Redraw
        }
    }
}

impl PagerContent for HelpContent {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
        let mut line = self.lines.get(index).cloned().unwrap_or_default();
        if let Some(m) = self.current.map(|current| self.matches[current])
            && m.line == index
        {
            let highlight = Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
            let mut offset = 0;
            let mut spans = Vec::new();
            for span in line.spans {
                let text = span.content.as_ref();
                let start = m.start.saturating_sub(offset).min(text.len());
                let end = m.end.saturating_sub(offset).min(text.len());
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

    fn on_resize(&mut self, width: usize, height: usize) -> bool {
        self.height = height;
        if width == self.width {
            return false;
        }
        let previous = self.current;
        self.lines = render_markdown(&self.source, Some("r"), Some(width));
        self.width = width;
        self.recompute_matches();
        self.current = previous.and_then(|index| {
            self.matches
                .len()
                .checked_sub(1)
                .map(|last| index.min(last))
        });
        self.update_status();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(content: &mut HelpContent, code: KeyCode) -> Option<PagerAction> {
        content.handle_key(code, KeyModifiers::NONE)
    }

    fn search(content: &mut HelpContent, query: &str) -> Option<PagerAction> {
        key(content, KeyCode::Char('/'));
        for ch in query.chars() {
            key(content, KeyCode::Char(ch));
        }
        key(content, KeyCode::Enter)
    }

    #[test]
    fn search_moves_forward_backward_and_wraps_in_both_directions() {
        let mut content = HelpContent::new("first needle\n\nsecond needle", 80, 24);
        assert_eq!(search(&mut content, "needle"), Some(PagerAction::Redraw));
        assert_eq!(content.current, Some(0));
        key(&mut content, KeyCode::Char('n'));
        assert_eq!(content.current, Some(1));
        key(&mut content, KeyCode::Char('n'));
        assert_eq!(content.current, Some(0));
        key(&mut content, KeyCode::Char('N'));
        assert_eq!(content.current, Some(1));
        key(&mut content, KeyCode::Char('N'));
        assert_eq!(content.current, Some(0));
    }

    #[test]
    fn search_starts_at_viewport_and_scrolls_only_for_hidden_matches() {
        let mut content = HelpContent::new("needle\n\nother\n\nneedle", 80, 4);
        content.prepare_render(2);
        assert_eq!(
            search(&mut content, "needle"),
            Some(PagerAction::ScrollTo(3))
        );
        assert_eq!(content.current, Some(1));
        content.prepare_render(3);
        assert_eq!(
            key(&mut content, KeyCode::Char('n')),
            Some(PagerAction::ScrollTo(0))
        );
        content.prepare_render(0);
        assert_eq!(
            key(&mut content, KeyCode::Char('N')),
            Some(PagerAction::ScrollTo(3))
        );
    }

    #[test]
    fn no_match_and_empty_search_report_feedback_without_moving() {
        let mut content = HelpContent::new("needle", 80, 24);
        assert_eq!(search(&mut content, ""), Some(PagerAction::Redraw));
        assert_eq!(content.feedback_message(), None);
        assert_eq!(search(&mut content, "absent"), Some(PagerAction::Redraw));
        assert!(content.matches.is_empty());
        assert!(
            content
                .feedback_message()
                .unwrap()
                .contains("No matches for: absent")
        );
        assert_eq!(
            key(&mut content, KeyCode::Char('N')),
            Some(PagerAction::Redraw)
        );
    }

    #[test]
    fn search_uses_rendered_text_across_styles_with_literal_case_sensitive_matching() {
        let mut content = HelpContent::new("**bold**needle Needle a.b axb", 80, 24);
        search(&mut content, "boldneedle");
        assert_eq!(content.matches.len(), 1);
        let line = content.render_line(0, 80);
        assert_eq!(line.to_string(), "boldneedle Needle a.b axb");
        assert_eq!(
            line.spans
                .iter()
                .filter(|s| s.style.bg == Some(Color::Yellow))
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "boldneedle"
        );
        search(&mut content, "**");
        assert!(content.matches.is_empty());
        search(&mut content, "needle");
        assert_eq!(content.matches.len(), 1);
        search(&mut content, "a.b");
        assert_eq!(content.matches.len(), 1);
    }

    #[test]
    fn unicode_matches_and_highlighting_preserve_text_and_style() {
        let mut content = HelpContent::new("前 **日本**語 後 日本語", 80, 24);
        search(&mut content, "日本語");
        assert_eq!(content.matches.len(), 2);
        let line = content.render_line(0, 80);
        assert_eq!(line.to_string(), "前 日本語 後 日本語");
        assert!(line.spans.iter().any(|s| s.content == "日本"
            && s.style.bg == Some(Color::Yellow)
            && s.style.add_modifier.contains(Modifier::BOLD)));
        assert!(
            line.spans
                .iter()
                .any(|s| s.content.contains("後 日本語") && s.style.bg != Some(Color::Yellow))
        );
    }

    #[test]
    fn wrapped_lines_are_searched_and_positions_recomputed_after_resize() {
        let mut content = HelpContent::new("alpha beta needle gamma needle", 80, 24);
        search(&mut content, "needle");
        assert_eq!(content.matches.len(), 2);
        assert!(content.matches.iter().all(|m| m.line == 0));
        assert!(content.on_resize(12, 24));
        assert_eq!(content.matches.len(), 2);
        assert!(content.matches.iter().all(|m| m.line > 0));
        for m in &content.matches {
            assert_eq!(&content.lines[m.line].to_string()[m.start..m.end], "needle");
        }
        search(&mut content, "beta needle");
        assert!(content.matches.is_empty());
        content.on_resize(80, 24);
        assert_eq!(content.matches.len(), 1);
    }

    #[test]
    fn input_consumes_pager_keys_edits_unicode_and_escape_restores_search() {
        let mut content = HelpContent::new("needle", 80, 24);
        search(&mut content, "needle");
        let previous = content.matches.clone();
        key(&mut content, KeyCode::Char('/'));
        for ch in "qjk/日本".chars() {
            assert_eq!(
                key(&mut content, KeyCode::Char(ch)),
                Some(PagerAction::Redraw)
            );
        }
        key(&mut content, KeyCode::Backspace);
        assert_eq!(content.input.as_deref(), Some("qjk/日"));
        for code in [KeyCode::Down, KeyCode::PageDown, KeyCode::Tab] {
            assert_eq!(key(&mut content, code), Some(PagerAction::Redraw));
        }
        assert_eq!(key(&mut content, KeyCode::Esc), Some(PagerAction::Redraw));
        assert_eq!(content.input, None);
        assert_eq!(content.query, "needle");
        assert_eq!(content.matches, previous);
        assert_eq!(content.current, Some(0));
        assert_eq!(key(&mut content, KeyCode::Esc), None);
        assert_eq!(key(&mut content, KeyCode::Enter), None);
    }

    #[test]
    fn enter_outside_input_keeps_the_page_and_search_unchanged() {
        let mut content = HelpContent::new("needle needle", 80, 24);
        assert_eq!(key(&mut content, KeyCode::Enter), None);
        search(&mut content, "needle");
        let current = content.current;
        let status = content.status.clone();
        let highlighted = content.render_line(0, 80);
        assert_eq!(key(&mut content, KeyCode::Enter), None);
        assert_eq!(content.current, current);
        assert_eq!(content.status, status);
        assert_eq!(content.render_line(0, 80), highlighted);
    }

    #[test]
    fn q_clears_committed_search_and_highlight_without_changing_scroll() {
        let mut content = HelpContent::new("needle", 80, 24);
        search(&mut content, "needle");
        content.prepare_render(3);
        assert!(content.feedback_message().unwrap().contains("n/N"));
        assert!(
            content
                .feedback_message()
                .unwrap()
                .contains("q clear search")
        );
        assert_eq!(
            key(&mut content, KeyCode::Char('q')),
            Some(PagerAction::Redraw)
        );
        assert_eq!(content.scroll_offset, 3);
        assert_eq!(content.feedback_message(), None);
        assert!(content.query.is_empty());
        assert!(content.matches.is_empty());
        assert_eq!(content.current, None);
        assert_eq!(content.render_line(0, 80), content.lines[0]);
        assert_eq!(key(&mut content, KeyCode::Char('q')), None);
        for ch in ['n', 'N'] {
            assert_eq!(key(&mut content, KeyCode::Char(ch)), None);
            assert_eq!(content.feedback_message(), None);
        }
    }

    #[test]
    fn no_match_search_can_be_cleared_and_does_not_advertise_match_navigation() {
        let mut content = HelpContent::new("needle", 80, 24);
        search(&mut content, "absent");
        assert!(!content.feedback_message().unwrap().contains("n/N"));
        assert_eq!(
            key(&mut content, KeyCode::Char('q')),
            Some(PagerAction::Redraw)
        );
        assert_eq!(content.feedback_message(), None);
        assert_eq!(key(&mut content, KeyCode::Char('q')), None);
    }

    #[test]
    fn empty_query_repeats_search_and_control_exit_keys_fall_through() {
        let mut content = HelpContent::new("needle needle", 80, 24);
        search(&mut content, "needle");
        search(&mut content, "");
        assert_eq!(content.current, Some(1));
        key(&mut content, KeyCode::Char('/'));
        for ch in ['c', 'd'] {
            assert_eq!(
                content.handle_key(KeyCode::Char(ch), KeyModifiers::CONTROL),
                None
            );
        }
        assert_eq!(content.input.as_deref(), Some(""));
    }
}
