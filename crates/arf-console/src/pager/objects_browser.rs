//! Interactive browser for .GlobalEnv objects (`:objects` command).

use super::text_utils::{display_width, pad_to_width, truncate_to_width};
use super::{MinimumSize, check_terminal_too_small, render_size_warning, with_alternate_screen};
use crate::fuzzy::fuzzy_match;
use arf_harp::{EnvEntry, workspace_snapshot};
use crossterm::{
    ExecutableCommand, cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    queue,
    style::Stylize,
    terminal::{self, BeginSynchronizedUpdate, ClearType, EndSynchronizedUpdate},
};
use std::io::{self, Write};
use std::time::Duration;

/// Minimum terminal size for the objects browser.
/// Must be at least FIXED_COLS + NAME_MIN = 51 + 15 = 66.
const MIN_SIZE: MinimumSize = MinimumSize { cols: 66, rows: 8 };

/// Fixed column widths (name is dynamic).
const CLASS_WIDTH: usize = 18;
const TYPE_WIDTH: usize = 12;
const SIZE_WIDTH: usize = 8;
/// Columns beyond name: cursor(1) + space(1) + " │ "(3)×3 + class + type + size + marker(2)
const FIXED_COLS: usize = 2 + 3 + CLASS_WIDTH + 3 + TYPE_WIDTH + 3 + SIZE_WIDTH + 2;
const NAME_MIN: usize = 15;

/// Result of running the objects browser.
#[derive(Debug, Clone)]
pub enum ObjectsBrowserResult {
    /// User exited without action.
    Cancelled,
}

struct ObjectsBrowser {
    entries: Vec<EnvEntry>,
    filtered: Vec<usize>,
    cursor: usize,
    scroll_offset: usize,
    filter_text: String,
    filter_active: bool,
    /// When true, dot-prefixed names are included (like ls(all.names=TRUE)).
    show_hidden: bool,
    feedback: Option<String>,
}

impl ObjectsBrowser {
    fn new(entries: Vec<EnvEntry>) -> Self {
        let n = entries.len();
        Self {
            entries,
            filtered: (0..n).collect(),
            cursor: 0,
            scroll_offset: 0,
            filter_text: String::new(),
            filter_active: false,
            show_hidden: false,
            feedback: None,
        }
    }

    fn reload(&mut self) {
        match workspace_snapshot(self.show_hidden) {
            Ok(entries) => {
                self.entries = entries;
                self.update_filter();
                self.feedback = Some(format!("{} objects", self.filtered.len()));
            }
            Err(e) => {
                self.feedback = Some(format!("Error: {e}"));
            }
        }
    }

    fn update_filter(&mut self) {
        if self.filter_text.is_empty() {
            self.filtered = (0..self.entries.len()).collect();
        } else {
            let query = &self.filter_text;
            self.filtered = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    fuzzy_match(query, &e.name).is_some()
                        || fuzzy_match(query, &e.class_label).is_some()
                })
                .map(|(i, _)| i)
                .collect();
        }
        if self.filtered.is_empty() {
            self.cursor = 0;
            self.scroll_offset = 0;
        } else {
            self.cursor = self.cursor.min(self.filtered.len() - 1);
            // Also clamp scroll_offset so the cursor stays in the visible window.
            // We don't know visible_rows here, so just ensure offset <= cursor.
            self.scroll_offset = self.scroll_offset.min(self.cursor);
        }
    }

    fn visible_rows(term_height: u16) -> usize {
        // Chrome: header(1) + filter(1) + sep(1) + col-header(1) + sep(1) + footer(1) = 6
        (term_height as usize).saturating_sub(6)
    }

    fn name_width(term_width: u16) -> usize {
        (term_width as usize)
            .saturating_sub(FIXED_COLS)
            .max(NAME_MIN)
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    fn move_down(&mut self, visible: usize) {
        if !self.filtered.is_empty() && self.cursor + 1 < self.filtered.len() {
            self.cursor += 1;
            if self.cursor >= self.scroll_offset + visible {
                self.scroll_offset = self.cursor - visible + 1;
            }
        }
    }

    fn move_page_up(&mut self, visible: usize) {
        self.cursor = self.cursor.saturating_sub(visible);
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
    }

    fn move_page_down(&mut self, visible: usize) {
        if self.filtered.is_empty() {
            return;
        }
        self.cursor = (self.cursor + visible).min(self.filtered.len() - 1);
        if self.cursor >= self.scroll_offset + visible {
            self.scroll_offset = self.cursor - visible + 1;
        }
    }

    fn move_to_top(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    fn move_to_bottom(&mut self, visible: usize) {
        if self.filtered.is_empty() {
            return;
        }
        self.cursor = self.filtered.len() - 1;
        self.scroll_offset = self.cursor.saturating_sub(visible - 1);
    }

    fn render(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));

        if let Some((c, r)) = check_terminal_too_small(&MIN_SIZE) {
            return render_size_warning(stdout, c, r, &MIN_SIZE);
        }

        let width = cols as usize;
        let visible = Self::visible_rows(rows);
        let name_w = Self::name_width(cols);

        queue!(
            stdout,
            BeginSynchronizedUpdate,
            cursor::MoveTo(0, 0),
            cursor::Hide
        )?;

        // ── Header ──────────────────────────────────────────────────────────
        let hidden_marker = if self.show_hidden { " [all]" } else { "" };
        let header = format!(
            "─ Objects [{}/{}]{} ─",
            self.filtered.len(),
            self.entries.len(),
            hidden_marker,
        );
        let padded = format!("{:─<width$}", header, width = width);
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        println!("\r{}", padded.dark_grey());

        // ── Filter line ──────────────────────────────────────────────────────
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        if self.filter_active {
            print!("\r  Filter: {}_", self.filter_text);
            let used = 11 + display_width(&self.filter_text);
            print!("{}", " ".repeat(width.saturating_sub(used)));
            println!();
        } else if self.filter_text.is_empty() {
            println!(
                "\r{}",
                pad_to_width("  Filter: (press / to filter)", width).dark_grey()
            );
        } else {
            println!(
                "\r{}",
                pad_to_width(&format!("  Filter: {}", self.filter_text), width)
            );
        }

        // ── Separator ────────────────────────────────────────────────────────
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        println!("\r{}", "─".repeat(width).dark_grey());

        // ── Column headers ───────────────────────────────────────────────────
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        let col_header = format!(
            "  {} │ {} │ {} │ {}",
            pad_to_width("Name", name_w),
            pad_to_width("Class", CLASS_WIDTH),
            pad_to_width("Type", TYPE_WIDTH),
            pad_to_width("Length", SIZE_WIDTH),
        );
        println!("\r{}", truncate_to_width(&col_header, width).dark_grey());

        // ── Entries ──────────────────────────────────────────────────────────
        for i in 0..visible {
            stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
            let idx = self.scroll_offset + i;
            if idx < self.filtered.len() {
                let entry_idx = self.filtered[idx];
                let entry = &self.entries[entry_idx];
                let is_current = idx == self.cursor;

                let name = truncate_to_width(&entry.name, name_w);
                let class = truncate_to_width(&entry.class_label, CLASS_WIDTH);
                let typ = truncate_to_width(&entry.type_label, TYPE_WIDTH);
                let size_str = format_size(entry.size);
                let marker = if entry.has_children { " >" } else { "  " };
                let cursor_mark = if is_current { ">" } else { " " };

                let content = format!(
                    "{} {} │ {} │ {} │ {}{}",
                    cursor_mark,
                    pad_to_width(&name, name_w),
                    pad_to_width(&class, CLASS_WIDTH),
                    pad_to_width(&typ, TYPE_WIDTH),
                    pad_to_width(&size_str, SIZE_WIDTH),
                    marker,
                );
                let line = pad_to_width(&content, width);

                if is_current {
                    println!("\r{}", line.reverse());
                } else if entry.is_active_binding || entry.is_promise {
                    println!("\r{}", line.dark_grey());
                } else {
                    println!("\r{line}");
                }
            } else {
                println!("\r{}", " ".repeat(width));
            }
        }

        // ── Footer ───────────────────────────────────────────────────────────
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        let footer_text = if let Some(msg) = &self.feedback {
            format!("─ {} ─", msg)
        } else {
            "─ ↑↓/jk navigate │ / filter │ a toggle hidden │ r refresh │ q exit ─".to_string()
        };
        let footer_display = truncate_to_width(&footer_text, width);
        let used = display_width(&footer_display);
        let padded_footer = format!(
            "{}{}",
            footer_display,
            "─".repeat(width.saturating_sub(used))
        );
        print!("\r{}", padded_footer.dark_grey());
        queue!(stdout, EndSynchronizedUpdate)?;
        stdout.flush()?;

        Ok(())
    }

    fn run(mut self, stdout: &mut io::Stdout) -> io::Result<ObjectsBrowserResult> {
        let mut needs_redraw = true;

        loop {
            let (_, rows) = terminal::size().unwrap_or((80, 24));
            let visible = Self::visible_rows(rows);

            if needs_redraw {
                self.render(stdout)?;
                needs_redraw = false;
            }

            if event::poll(Duration::from_millis(100))? {
                self.feedback = None;

                match event::read()? {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }
                        needs_redraw = true;

                        if self.filter_active {
                            match (key.code, key.modifiers) {
                                (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
                                    self.filter_active = false;
                                }
                                (KeyCode::Backspace, _) => {
                                    self.filter_text.pop();
                                    self.update_filter();
                                }
                                (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                    self.filter_text.clear();
                                    self.update_filter();
                                }
                                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                    self.filter_text.push(c);
                                    self.update_filter();
                                }
                                _ => {}
                            }
                        } else {
                            match (key.code, key.modifiers) {
                                (KeyCode::Esc, _)
                                | (KeyCode::Char('q'), KeyModifiers::NONE)
                                | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                                | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                    return Ok(ObjectsBrowserResult::Cancelled);
                                }
                                (KeyCode::Char('/'), KeyModifiers::NONE) => {
                                    self.filter_active = true;
                                }
                                (KeyCode::Up, _)
                                | (KeyCode::Char('k'), KeyModifiers::NONE)
                                | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                                    self.move_up();
                                }
                                (KeyCode::Down, _)
                                | (KeyCode::Char('j'), KeyModifiers::NONE)
                                | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                                    self.move_down(visible);
                                }
                                (KeyCode::PageUp, _)
                                | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                                    self.move_page_up(visible);
                                }
                                (KeyCode::PageDown, _)
                                | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                                    self.move_page_down(visible);
                                }
                                (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                                    self.move_to_top();
                                }
                                (KeyCode::End, _) | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                                    self.move_to_bottom(visible);
                                }
                                (KeyCode::Char('a'), KeyModifiers::NONE) => {
                                    self.show_hidden = !self.show_hidden;
                                    self.reload();
                                }
                                (KeyCode::Char('r'), KeyModifiers::NONE) => {
                                    self.reload();
                                }
                                _ => {
                                    needs_redraw = false;
                                }
                            }
                        }
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            needs_redraw = true;
                            self.move_up();
                        }
                        MouseEventKind::ScrollDown => {
                            needs_redraw = true;
                            self.move_down(visible);
                        }
                        _ => {}
                    },
                    Event::Resize(_, _) => {
                        needs_redraw = true;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn format_size(size: Option<i64>) -> String {
    match size {
        None => String::new(),
        Some(n) => {
            if n >= 1_000_000 {
                format!("{:.1}M", n as f64 / 1_000_000.0)
            } else if n >= 1_000 {
                format!("{:.1}k", n as f64 / 1_000.0)
            } else {
                n.to_string()
            }
        }
    }
}

/// Run the objects browser, displaying .GlobalEnv contents.
pub fn run_objects_browser() -> io::Result<ObjectsBrowserResult> {
    // Default: hide dot-prefixed names (matches R's ls() default)
    let entries = workspace_snapshot(false).unwrap_or_default();
    let browser = ObjectsBrowser::new(entries);
    with_alternate_screen(|| {
        let mut stdout = io::stdout();
        browser.run(&mut stdout)
    })
}
