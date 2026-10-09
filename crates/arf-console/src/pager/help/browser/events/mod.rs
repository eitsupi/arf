//! Terminal event handling and input backlog ordering for the help browser.

#[cfg(test)]
mod tests;

use super::super::search::SearchWorker;
use super::HelpBrowser;
use super::render::visible_result_rows;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserAction {
    Continue,
    Open(usize),
    Exit,
}

pub(super) struct EventHandling {
    pub(super) action: BrowserAction,
    pub(super) redraw: bool,
}

pub(super) struct EventDrain {
    pub(super) action: BrowserAction,
    pub(super) events_read: usize,
    pub(super) redraw: bool,
}

pub(super) fn drain_help_events(
    mut next_event: impl FnMut() -> io::Result<Option<Event>>,
    mut handle_event: impl FnMut(Event) -> io::Result<EventHandling>,
) -> io::Result<EventDrain> {
    let mut drained = EventDrain {
        action: BrowserAction::Continue,
        events_read: 0,
        redraw: false,
    };
    while drained.events_read < 32 {
        let Some(event) = next_event()? else {
            break;
        };
        drained.events_read += 1;
        let handling = handle_event(event)?;
        drained.redraw |= handling.redraw;
        drained.action = handling.action;
        if handling.action != BrowserAction::Continue {
            break;
        }
    }
    Ok(drained)
}

pub(super) fn poll_backlog_before_results(
    action: BrowserAction,
    poll_input: impl FnOnce() -> io::Result<bool>,
    apply_results: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    if poll_input()? {
        return Ok(true);
    }
    if action == BrowserAction::Continue {
        apply_results()?;
    }
    Ok(false)
}

impl HelpBrowser {
    pub(super) fn handle_event(
        &mut self,
        event: Event,
        too_small: bool,
        worker: &SearchWorker,
    ) -> io::Result<EventHandling> {
        let continue_with = |redraw| EventHandling {
            action: BrowserAction::Continue,
            redraw,
        };
        match event {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Ok(continue_with(false));
                }
                if too_small {
                    match (key.code, key.modifiers) {
                        (KeyCode::Esc, _)
                        | (KeyCode::Char('q'), KeyModifiers::NONE)
                        | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                        | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            self.cancel_search(worker)?;
                            return Ok(EventHandling {
                                action: BrowserAction::Exit,
                                redraw: true,
                            });
                        }
                        _ => return Ok(continue_with(true)),
                    }
                }

                match (key.code, key.modifiers) {
                    // Exit
                    (KeyCode::Esc, _)
                    | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                        self.cancel_search(worker)?;
                        return Ok(EventHandling {
                            action: BrowserAction::Exit,
                            redraw: true,
                        });
                    }

                    (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                        self.move_selection(-1, visible_result_rows());
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                        self.move_selection(1, visible_result_rows());
                    }

                    (KeyCode::Enter, _) | (KeyCode::Tab, _) => {
                        if self.search_pending() {
                            self.remember_pending_open();
                        } else if let Some(&(index, _)) = self.filtered.get(self.selected) {
                            return Ok(EventHandling {
                                action: BrowserAction::Open(index),
                                redraw: true,
                            });
                        }
                    }

                    (KeyCode::Backspace, _) => {
                        self.backspace_query();
                    }
                    (KeyCode::Delete, _) => {
                        self.delete_query_char();
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                        self.clear_query();
                    }
                    (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                        self.insert_query_char(c);
                    }
                    (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                        self.move_cursor(self.cursor_pos.saturating_sub(1));
                    }
                    (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                        self.move_cursor((self.cursor_pos + 1).min(self.query.chars().count()));
                    }
                    (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                        self.move_cursor(0);
                    }
                    (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                        self.move_cursor(self.query.chars().count());
                    }

                    _ => {}
                }
                Ok(continue_with(true))
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.move_selection(-1, visible_result_rows());
                    Ok(continue_with(true))
                }
                MouseEventKind::ScrollDown => {
                    self.move_selection(1, visible_result_rows());
                    Ok(continue_with(true))
                }
                _ => Ok(continue_with(false)),
            },
            Event::Resize(_, _) => Ok(continue_with(true)),
            _ => Ok(continue_with(false)),
        }
    }
}
