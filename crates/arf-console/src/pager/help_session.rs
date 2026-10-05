//! A single pager session for help pages, link selection, and back history.

use super::help::HelpPageSelectorState;
use super::help_content::HelpContent;
use super::{PagerAction, PagerContent};
use arf_harp::help::{HelpResolution, HelpTargetResolver, HelpTopic};
use arf_harp::help_bridge::PreparedHelpPage;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

struct HelpPageState {
    page: PreparedHelpPage,
    title: String,
    content: HelpContent,
}

impl HelpPageState {
    fn new(page: PreparedHelpPage, width: usize, height: usize) -> Self {
        Self {
            title: super::help::help_page_title(&page.package, &page.display_topic),
            content: HelpContent::new(&page.markdown, width, height),
            page,
        }
    }
}

enum CandidatePages {
    Prepared(Vec<PreparedHelpPage>),
    Topics(Vec<HelpTopic>),
}

struct HelpCandidates {
    pages: CandidatePages,
    labels: Vec<String>,
    state: HelpPageSelectorState,
}

impl HelpCandidates {
    fn new(pages: CandidatePages) -> Self {
        let labels = match &pages {
            CandidatePages::Prepared(pages) => pages
                .iter()
                .map(|page| {
                    format!(
                        "{}::{}  [{}] ({})",
                        page.package,
                        page.display_topic,
                        page.help_key,
                        page.package_dir.display()
                    )
                })
                .collect::<Vec<_>>(),
            CandidatePages::Topics(topics) => topics
                .iter()
                .map(|topic| {
                    format!(
                        "{}  [{}] {} ({})",
                        topic.qualified_name(),
                        topic.help_key.as_deref().unwrap_or("alias"),
                        topic.title,
                        topic.package_dir.display()
                    )
                })
                .collect(),
        };
        Self {
            state: HelpPageSelectorState::new(labels.len()),
            pages,
            labels,
        }
    }
}

pub(super) struct HelpViewer {
    current: Option<HelpPageState>,
    history: Vec<HelpPageState>,
    candidates: Option<HelpCandidates>,
    resolver: HelpTargetResolver,
    width: usize,
    height: usize,
    scroll_offset: usize,
    pending_scroll: Option<usize>,
    message: Option<String>,
}

impl HelpViewer {
    /// The caller supplies library paths before entering the alternate screen.
    /// Multiple initial pages require explicit selection in this same loop.
    pub(super) fn new(
        mut pages: Vec<PreparedHelpPage>,
        libraries: Vec<String>,
        width: usize,
        height: usize,
    ) -> Self {
        assert!(!pages.is_empty(), "help viewer requires at least one page");
        let (current, candidates) = if pages.len() == 1 {
            (
                Some(HelpPageState::new(pages.remove(0), width, height)),
                None,
            )
        } else {
            (
                None,
                Some(HelpCandidates::new(CandidatePages::Prepared(pages))),
            )
        };
        Self {
            current,
            candidates,
            history: Vec::new(),
            resolver: HelpTargetResolver::new(libraries),
            width,
            height,
            scroll_offset: 0,
            pending_scroll: None,
            message: None,
        }
    }

    fn show_page(&mut self, page: PreparedHelpPage) -> PagerAction {
        let next = HelpPageState::new(page, self.width, self.height);
        if let Some(previous) = self.current.replace(next) {
            self.history.push(previous);
        }
        self.candidates = None;
        self.message = None;
        PagerAction::ScrollTo(0)
    }

    fn follow_link(&mut self) -> PagerAction {
        let Some(current) = &self.current else {
            return PagerAction::Redraw;
        };
        let Some(target) = current.content.selected_target() else {
            return PagerAction::Redraw;
        };
        match self.resolver.resolve(&current.page, &target) {
            Ok(HelpResolution::Page(page)) => self.show_page(page),
            Ok(HelpResolution::Candidates(topics)) => {
                self.candidates = Some(HelpCandidates::new(CandidatePages::Topics(topics)));
                PagerAction::ScrollTo(0)
            }
            Ok(HelpResolution::NotFound) => {
                self.message = Some(format!("Help topic not found: {}", target.topic));
                PagerAction::Redraw
            }
            Err(error) => {
                self.message = Some(format!("Unable to open help topic: {error}"));
                PagerAction::Redraw
            }
        }
    }

    fn back(&mut self) -> PagerAction {
        let Some(mut previous) = self.history.pop() else {
            return PagerAction::Redraw;
        };
        let offset = previous.content.restore_viewport(self.width, self.height);
        self.current = Some(previous);
        self.message = None;
        PagerAction::ScrollTo(offset)
    }

    fn cancel_candidates(&mut self) -> PagerAction {
        self.candidates = None;
        self.current.as_ref().map_or(PagerAction::Exit, |page| {
            PagerAction::ScrollTo(page.content.scroll_offset())
        })
    }

    fn handle_candidates(&mut self, code: KeyCode, modifiers: KeyModifiers) -> PagerAction {
        if matches!(code, KeyCode::Esc | KeyCode::Backspace)
            || (code == KeyCode::Char('q') && modifiers == KeyModifiers::NONE)
            || (code == KeyCode::Left && modifiers == KeyModifiers::ALT)
        {
            return self.cancel_candidates();
        }
        let candidates = self.candidates.as_mut().expect("candidate mode is active");
        if code == KeyCode::Enter {
            let Some(index) = candidates.state.confirm() else {
                return PagerAction::Redraw;
            };
            let candidates = self.candidates.take().expect("candidate mode is active");
            let page = match candidates.pages {
                CandidatePages::Prepared(mut pages) => Ok(pages.remove(index)),
                CandidatePages::Topics(topics) => {
                    HelpTargetResolver::prepare_candidate(&topics[index])
                }
            };
            return match page {
                Ok(page) => self.show_page(page),
                Err(error) => {
                    // Keep the previous page and its viewport when preparation fails.
                    self.message = Some(format!("Unable to open help topic: {error}"));
                    self.current.as_ref().map_or(PagerAction::Exit, |page| {
                        PagerAction::ScrollTo(page.content.scroll_offset())
                    })
                }
            };
        }
        match (code, modifiers) {
            (KeyCode::Down, _)
            | (KeyCode::Char('j'), KeyModifiers::NONE)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL)
            | (KeyCode::Tab, KeyModifiers::NONE) => {
                candidates.state.move_down();
            }
            (KeyCode::Up, _)
            | (KeyCode::Char('k'), KeyModifiers::NONE)
            | (KeyCode::Char('p'), KeyModifiers::CONTROL)
            | (KeyCode::BackTab, _)
            | (KeyCode::Tab, KeyModifiers::SHIFT) => {
                candidates.state.move_up();
            }
            _ => return PagerAction::Redraw,
        }
        let line = candidates.state.selected.unwrap_or(0);
        let rows = self.height.saturating_sub(2).max(1);
        if line < self.scroll_offset {
            PagerAction::ScrollTo(line)
        } else if line >= self.scroll_offset.saturating_add(rows) {
            PagerAction::ScrollTo(line.saturating_sub(rows - 1))
        } else {
            PagerAction::Redraw
        }
    }
}

impl PagerContent for HelpViewer {
    fn title(&self) -> Option<&str> {
        if self.candidates.is_some() {
            Some("Select R help page")
        } else {
            self.current.as_ref().map(|page| page.title.as_str())
        }
    }

    fn line_count(&self) -> usize {
        if let Some(candidates) = &self.candidates {
            candidates.labels.len()
        } else {
            self.current
                .as_ref()
                .map_or(0, |page| page.content.line_count())
        }
    }

    fn render_line(&self, index: usize, width: usize) -> Line<'static> {
        if let Some(candidates) = &self.candidates {
            let selected = candidates.state.selected == Some(index);
            let prefix = if selected { ">" } else { " " };
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::styled(
                format!("{prefix} [{}] {}", index + 1, candidates.labels[index]),
                style,
            )
        } else {
            self.current
                .as_ref()
                .map_or_else(Line::default, |page| page.content.render_line(index, width))
        }
    }

    fn prepare_render(&mut self, offset: usize) {
        self.scroll_offset = offset;
        if self.candidates.is_none()
            && let Some(page) = &mut self.current
        {
            page.content.prepare_render(offset);
        }
    }

    fn take_scroll_request(&mut self) -> Option<usize> {
        self.pending_scroll.take()
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<PagerAction> {
        if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c' | 'd')) {
            return None;
        }
        self.message = None;
        if self.candidates.is_some() {
            return Some(self.handle_candidates(code, modifiers));
        }
        let page = self.current.as_mut()?;
        // Search input consumes navigation keys, including Tab, Enter and Backspace.
        if page.content.is_search_input() {
            return page.content.handle_key(code, modifiers);
        }
        match (code, modifiers) {
            (KeyCode::Tab, KeyModifiers::NONE) => Some(page.content.select_link(true)),
            (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::SHIFT) => {
                Some(page.content.select_link(false))
            }
            (KeyCode::Enter, _) => Some(self.follow_link()),
            (KeyCode::Backspace, KeyModifiers::NONE) | (KeyCode::Left, KeyModifiers::ALT) => {
                Some(self.back())
            }
            _ => page.content.handle_key(code, modifiers),
        }
    }

    fn feedback_message(&self) -> Option<&str> {
        if self.candidates.is_some() {
            return Some("↑↓/jk move  Enter open  q/Esc cancel");
        }
        if let Some(message) = &self.message {
            return Some(message);
        }
        let page = self.current.as_ref()?;
        if let Some(status) = page.content.feedback_message() {
            return Some(status);
        }
        if self.width < 70 {
            // Keep exit hints visible when the terminal cannot fit all bindings.
            return Some(
                match (
                    page.content.has_help_links(),
                    page.content.selected_target().is_some(),
                    self.history.is_empty(),
                ) {
                    (_, true, false) => "q/Esc exit  Enter open  Backspace back",
                    (_, true, true) => "q/Esc exit  Enter open  Tab links",
                    (true, false, false) => "q/Esc exit  Backspace back  Tab links",
                    (true, false, true) => "q/Esc exit  / search  Tab links",
                    (false, _, false) => "q/Esc exit  / search  Backspace back",
                    (false, _, true) => "q/Esc exit  / search  ↑↓/jk scroll",
                },
            );
        }
        match (
            page.content.has_help_links(),
            page.content.selected_target().is_some(),
            self.history.is_empty(),
        ) {
            (true, true, false) => {
                Some("Tab/S-Tab link  Enter open  Backspace back  / search  q/Esc exit")
            }
            (true, true, true) => Some("Tab/S-Tab link  Enter open  / search  q/Esc exit"),
            (true, false, false) => {
                Some("↑↓/jk scroll  Tab/S-Tab link  Backspace back  / search  q/Esc exit")
            }
            (true, false, true) => Some("↑↓/jk scroll  Tab/S-Tab link  / search  q/Esc exit"),
            (false, _, false) => Some("↑↓/jk scroll  Backspace back  / search  q/Esc exit"),
            (false, _, true) => Some("↑↓/jk scroll  / search  q/Esc exit"),
        }
    }

    fn on_resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        if let Some(page) = &mut self.current {
            let offset = page.content.restore_viewport(width, height);
            if self.candidates.is_none() {
                self.pending_scroll = Some(offset);
            }
        }
    }
}

#[cfg(test)]
mod tests;
