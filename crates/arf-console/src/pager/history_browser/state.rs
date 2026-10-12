//! Browser state: entries, selection, cursor movement and deletion.

use super::filter::HistoryFilter;
use super::render::visible_result_rows;
use super::{HistoryDbMode, load_history};
use crate::history::HistoryStore;
use crate::pager::TextScrollState;
use reedline::{HistoryItem, HistoryItemId};
use std::io;

/// A history item with selection state.
pub(super) struct BrowsableHistoryItem {
    /// The actual history item.
    pub(super) item: HistoryItem,
    /// Whether this item is selected for deletion.
    pub(super) selected: bool,
}

/// Interactive history browser.
pub(super) struct HistoryBrowser {
    /// All loaded history entries.
    pub(super) entries: Vec<BrowsableHistoryItem>,
    /// Filtered entries as (index, score) pairs.
    pub(super) filtered: Vec<(usize, u32)>,
    /// Current filter state.
    pub(super) filter: HistoryFilter,
    /// Cursor position in the list.
    pub(super) cursor: usize,
    /// Scroll offset for the list.
    pub(super) scroll_offset: usize,
    /// Feedback message to display.
    pub(super) feedback_message: Option<String>,
    /// Database mode (R or Shell).
    pub(super) db_mode: HistoryDbMode,
    /// The already-open arf-owned history store.
    pub(super) store: HistoryStore,
    /// Scroll animation state for the selected item's long text.
    pub(super) text_scroll: TextScrollState,
    /// Whether we're showing the delete confirmation dialog.
    pub(super) show_delete_dialog: bool,
    /// Whether filter input mode is active.
    /// When true, all character input goes to the filter text.
    /// When false, single-char keybindings (q, d, y, etc.) work as navigation/commands.
    pub(super) filter_active: bool,
    /// Cached count of selected entries (maintained by toggle/select/unselect/delete).
    pub(super) cached_selected_count: usize,
}

impl HistoryBrowser {
    /// Create a new history browser.
    pub(super) fn new(
        entries: Vec<HistoryItem>,
        db_mode: HistoryDbMode,
        store: HistoryStore,
    ) -> Self {
        let browsable: Vec<BrowsableHistoryItem> = entries
            .into_iter()
            .map(|item| BrowsableHistoryItem {
                item,
                selected: false,
            })
            .collect();
        let filtered: Vec<(usize, u32)> =
            browsable.iter().enumerate().map(|(i, _)| (i, 0)).collect();

        HistoryBrowser {
            entries: browsable,
            filtered,
            filter: HistoryFilter::default(),
            cursor: 0,
            scroll_offset: 0,
            feedback_message: None,
            db_mode,
            store,
            text_scroll: TextScrollState::new(),
            show_delete_dialog: false,
            filter_active: false,
            cached_selected_count: 0,
        }
    }

    /// Count of currently selected items (cached).
    pub(super) fn selected_count(&self) -> usize {
        self.cached_selected_count
    }

    /// Toggle selection for the item at cursor.
    pub(super) fn toggle_selection(&mut self) {
        if let Some(&(idx, _)) = self.filtered.get(self.cursor) {
            let entry = &mut self.entries[idx];
            entry.selected = !entry.selected;
            if entry.selected {
                self.cached_selected_count += 1;
            } else {
                self.cached_selected_count = self.cached_selected_count.saturating_sub(1);
            }
        }
    }

    /// Select all visible (filtered) items.
    pub(super) fn select_all_visible(&mut self) {
        for &(idx, _) in &self.filtered {
            if !self.entries[idx].selected {
                self.entries[idx].selected = true;
                self.cached_selected_count += 1;
            }
        }
    }

    /// Unselect all items.
    pub(super) fn unselect_all(&mut self) {
        for entry in &mut self.entries {
            entry.selected = false;
        }
        self.cached_selected_count = 0;
    }

    /// Delete all selected items from the database.
    ///
    /// Deletes through the already-open arf-owned store. The browser owns no
    /// database connection and never holds the store lock during UI input.
    pub(super) fn delete_selected(&mut self) -> io::Result<()> {
        // Collect IDs to delete
        let ids_to_delete: Vec<i64> = self
            .entries
            .iter()
            .filter(|e| e.selected)
            .filter_map(|e| e.item.id)
            .map(|id| id.0)
            .collect();

        if ids_to_delete.is_empty() {
            return Ok(());
        }

        let ids: Vec<HistoryItemId> = ids_to_delete
            .iter()
            .copied()
            .map(HistoryItemId::new)
            .collect();
        if let Err(error) = self.store.delete_many(&ids) {
            // A backend error can occur after a prefix of ids was deleted.
            // Reload from the owner before returning so the UI cannot retain
            // rows that no longer exist (or hide rows that were not deleted).
            self.entries = load_history(&self.store)?
                .into_iter()
                .map(|item| BrowsableHistoryItem {
                    item,
                    selected: false,
                })
                .collect();
            self.cached_selected_count = 0;
            self.update_filter();
            return Err(io::Error::other(error));
        }

        // Remove deleted entries from our list
        let feedback = format!("Deleted {} entries", ids_to_delete.len());
        self.entries.retain(|e| !e.selected);
        self.cached_selected_count = 0;

        // Rebuild filtered list
        self.update_filter();

        // Adjust cursor if needed
        if self.cursor >= self.filtered.len() && !self.filtered.is_empty() {
            self.cursor = self.filtered.len() - 1;
        }

        self.feedback_message = Some(feedback);
        Ok(())
    }

    /// Move cursor up by one row.
    pub(super) fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    /// Move cursor down by one row.
    pub(super) fn move_cursor_down(&mut self) {
        let visible_rows = visible_result_rows();
        if self.cursor + 1 < self.filtered.len() {
            self.cursor += 1;
            if self.cursor >= self.scroll_offset + visible_rows {
                self.scroll_offset = self.cursor - visible_rows + 1;
            }
        }
    }

    /// Move cursor up by one page.
    pub(super) fn move_page_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let page_size = visible_result_rows();
        self.cursor = self.cursor.saturating_sub(page_size);
        self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
    }

    /// Move cursor down by one page.
    pub(super) fn move_page_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let page_size = visible_result_rows();
        let max_cursor = self.filtered.len() - 1;
        self.cursor = (self.cursor + page_size).min(max_cursor);
        let max_scroll = self.filtered.len().saturating_sub(page_size);
        self.scroll_offset = (self.scroll_offset + page_size).min(max_scroll);
    }

    /// Move cursor to the first entry.
    pub(super) fn move_to_top(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Move cursor to the last entry.
    pub(super) fn move_to_bottom(&mut self) {
        if !self.filtered.is_empty() {
            self.cursor = self.filtered.len() - 1;
            let visible_rows = visible_result_rows();
            if self.cursor >= visible_rows {
                self.scroll_offset = self.cursor - visible_rows + 1;
            }
        }
    }

    /// Get the command line at the current cursor position.
    pub(super) fn current_command(&self) -> Option<&str> {
        self.filtered
            .get(self.cursor)
            .map(|&(idx, _)| self.entries[idx].item.command_line.as_str())
    }

    /// Update the text scroll animation state.
    pub(super) fn update_text_scroll(&mut self) -> bool {
        self.text_scroll.update(self.cursor)
    }
}
