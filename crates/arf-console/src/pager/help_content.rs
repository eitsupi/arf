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
mod tests {
    use super::*;

    #[test]
    fn link_focus_skips_non_help_targets_wraps_and_scrolls_into_view() {
        let source = format!(
            "[site](https://example.com) [broken](x-r-help:base/)\n\n[first](x-r-help:/mean)\n\n{}\n\n[last](x-r-help:stats/lm)",
            "paragraph\n\n".repeat(12)
        );
        let mut content = HelpContent::new(&source, 30, 6);
        assert_eq!(content.selected_target(), None);
        assert!(content.has_help_links());
        content.select_link(true);
        assert_eq!(content.selected_target().unwrap().topic, "mean");
        assert!(matches!(
            content.select_link(true),
            PagerAction::ScrollTo(_)
        ));
        assert_eq!(content.selected_target().unwrap().topic, "lm");
        content.select_link(true);
        assert_eq!(content.selected_target().unwrap().topic, "mean");
        content.select_link(false);
        assert_eq!(content.selected_target().unwrap().topic, "lm");
        let mut content = HelpContent::new("[site](https://example.com)", 30, 6);
        assert!(!content.has_help_links());
        assert_eq!(content.select_link(true), PagerAction::Redraw);
        assert_eq!(content.selected_target(), None);
    }

    #[test]
    fn wrapped_unicode_link_labels_are_highlighted_and_keep_independent_identities() {
        let mut content = HelpContent::new(
            "[**日本語** with long label](x-r-help:/mean) and [日本語](x-r-help:/mean)",
            12,
            24,
        );
        content.select_link(true);
        let first = content.selected_link;
        assert_eq!(first, Some(0));
        assert!(content.document.links[0].fragments.len() > 1);
        for fragment in &content.document.links[0].fragments {
            let line = content.render_line(fragment.line, 12);
            let highlighted: String = line
                .spans
                .iter()
                .filter(|s| s.style.bg == Some(Color::Cyan))
                .map(|s| s.content.as_ref())
                .collect();
            let plain = content.document.lines[fragment.line].to_string();
            assert!(highlighted.contains(&plain[fragment.byte_range.clone()]));
        }
        content.on_resize(30, 24);
        assert_eq!(content.selected_link, first);
        content.select_link(true);
        assert_eq!(content.selected_link, Some(1));
        assert_eq!(content.selected_target().unwrap().topic, "mean");
        content.select_link(false);
        assert_eq!(content.selected_link, first);
    }

    #[test]
    fn rd_link_targets_survive_rendering_and_resize() {
        let rd = include_str!("../../tests/fixtures/help_links.Rd");
        let source = arf_harp::help::rd_source_to_markdown(rd).unwrap();
        let expected = [
            (None, "mean"),
            (Some("stats"), "lm"),
            (None, "mean"),
            (None, "["),
            (None, "[["),
            (None, "%/%"),
            (None, "%in%"),
            (None, "/"),
            (Some("base"), "%/%"),
            (Some("base"), "mean"),
        ];
        let mut content = HelpContent::new(&source, 80, 24);
        for width in [8, 24, 80] {
            content.on_resize(width, 24);
            assert_eq!(content.document.links.len(), expected.len() + 1);
            for (link, (package, topic)) in content.document.links.iter().zip(expected) {
                assert_eq!(
                    HelpTarget::from_uri(&link.destination),
                    Some(HelpTarget {
                        package: package.map(str::to_owned),
                        topic: topic.to_owned(),
                    }),
                );
                assert!(!link.text_ranges.is_empty());
                assert!(!link.fragments.is_empty());
                for fragment in &link.fragments {
                    let line = &content.document.lines[fragment.line];
                    let text: String = line
                        .spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect();
                    assert!(!text[fragment.byte_range.clone()].is_empty());
                    assert!(fragment.columns.start < fragment.columns.end);
                }
            }
            let label = &content.document.links[2];
            assert_eq!(label.label, "an average with a longer label");
            if width == 8 {
                assert!(label.fragments.len() > 1);
            }
            let website = content.document.links.last().unwrap();
            assert_eq!(website.destination, "https://example.com/docs");
            assert_eq!(HelpTarget::from_uri(&website.destination), None);
            assert!(
                content
                    .document
                    .text
                    .iter()
                    .all(|t| !t.text.contains("x-r-help:"))
            );
        }
    }

    #[test]
    fn invalid_help_destinations_remain_readable() {
        let content = HelpContent::new(
            "[broken](x-r-help:base/) and [mean](x-r-help:/mean)",
            80,
            24,
        );
        assert_eq!(
            content.document.text[0].text,
            "broken (x-r-help:base/) and mean"
        );
    }

    #[test]
    fn help_links_keep_metadata_without_exposing_internal_uris() {
        let source = "Read [**mean**](x-r-help:base/mean) and [docs](https://example.com).";
        let mut content = HelpContent::new(source, 80, 24);
        assert_eq!(
            content.document.text[0].text,
            "Read mean and docs (https://example.com)."
        );
        assert_eq!(content.document.links[0].destination, "x-r-help:base/mean");
        assert_eq!(content.document.links[1].destination, "https://example.com");
        for width in [12, 40, 80] {
            content.on_resize(width, 24);
            assert_eq!(content.document.links.len(), 2);
            assert_eq!(content.document.links[0].label, "mean");
            assert!(!content.document.links[0].fragments.is_empty());
            assert!(
                !content
                    .document
                    .text
                    .iter()
                    .any(|t| t.text.contains("x-r-help:"))
            );
        }
    }

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

    fn highlighted_text(content: &HelpContent) -> String {
        (0..content.line_count())
            .flat_map(|index| content.render_line(index, content.width).spans)
            .filter(|span| span.style.bg == Some(Color::Yellow))
            .map(|span| span.content.into_owned())
            .collect()
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
    fn logical_matches_survive_resize_and_map_to_wrapped_lines() {
        let mut content = HelpContent::new("alpha beta needle gamma needle", 80, 24);
        search(&mut content, "needle");
        assert_eq!(content.matches.len(), 2);
        let matches = content.matches.clone();
        assert!(content.matches.iter().all(|m| content.match_line(*m) == 0));
        content.on_resize(12, 24);
        assert_eq!(content.matches, matches);
        assert_eq!(content.current, None);
        assert!(content.matches.iter().all(|m| content.match_line(*m) > 0));
        for m in &content.matches {
            for (line, start, end) in content.visual_ranges(*m) {
                assert_eq!(
                    &content.document.lines[line].to_string()[start..end],
                    "needle"
                );
            }
        }
        search(&mut content, "beta needle");
        assert_eq!(content.matches.len(), 1);
        let matches = content.matches.clone();
        for width in [80, 12, 5, 1, 0] {
            content.on_resize(width, 24);
            assert_eq!(content.matches, matches);
            key(&mut content, KeyCode::Char('n'));
            assert_eq!(highlighted_text(&content).replace(' ', ""), "betaneedle");
        }
    }

    #[test]
    fn wrapped_unicode_phrases_keep_identity_and_highlight_in_lists_and_quotes() {
        for source in ["- 前 **日本**語 needle 後", "> 前 **日本**語 needle 後"] {
            let mut content = HelpContent::new(source, 80, 24);
            search(&mut content, "日本語 needle");
            let matches = content.matches.clone();
            assert_eq!(matches.len(), 1);
            for width in [8, 6, 2, 80] {
                content.on_resize(width, 24);
                assert_eq!(content.matches, matches);
                key(&mut content, KeyCode::Char('n'));
                assert_eq!(highlighted_text(&content).replace(' ', ""), "日本語needle");
            }
        }
    }

    #[test]
    fn table_cell_phrases_survive_wrapping_and_map_past_unicode_columns() {
        let source = "| Left | Right |\n| --- | --- |\n| 日本 | alpha **beta** needle gamma |";
        let mut content = HelpContent::new(source, 80, 24);
        search(&mut content, "beta needle");
        let matches = content.matches.clone();
        assert_eq!(matches.len(), 1);
        for width in [22, 16, 80] {
            content.on_resize(width, 24);
            assert_eq!(content.matches, matches);
            key(&mut content, KeyCode::Char('n'));
            assert_eq!(highlighted_text(&content).replace(' ', ""), "betaneedle");
        }
    }

    #[test]
    fn search_does_not_cross_hard_breaks_paragraphs_code_lines_or_table_cells() {
        for source in [
            "beta  \nneedle",
            "beta\n\nneedle",
            "```\nbeta\nneedle\n```",
            "| Left | Right |\n| --- | --- |\n| beta | needle |",
            "| Text |\n| --- |\n| beta<br>needle |",
        ] {
            let mut content = HelpContent::new(source, 80, 24);
            search(&mut content, "beta needle");
            assert!(content.matches.is_empty());
            content.on_resize(12, 24);
            assert!(content.matches.is_empty());
        }
    }

    #[test]
    fn resize_clears_selection_and_keeps_query_and_count_for_next_navigation() {
        let mut content = HelpContent::new("needle needle", 80, 24);
        search(&mut content, "needle");
        key(&mut content, KeyCode::Char('n'));
        assert_eq!(content.current, Some(1));
        content.on_resize(80, 4);
        assert_eq!(content.current, None);
        assert_eq!(content.query, "needle");
        assert!(content.feedback_message().unwrap().contains("[0/2]"));
        assert_eq!(highlighted_text(&content), "");
        content.on_resize(6, 4);
        content.prepare_render(1);
        key(&mut content, KeyCode::Char('n'));
        assert_eq!(content.current, Some(1));
        assert_eq!(highlighted_text(&content), "needle");
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
        assert_eq!(content.render_line(0, 80), content.document.lines[0]);
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
