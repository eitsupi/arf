use reedline::HistoryItemId;
use std::collections::VecDeque;

/// Queue for the sponge-like "forget failed commands" feature.
///
/// This mimics fish shell's sponge plugin behavior:
/// - Every command adds an entry: `Some(id)` for failures, `None` for successes
/// - When queue length exceeds `delay`, the oldest entry is removed
/// - If that entry was a failed command (`Some(id)`), it should be deleted from history
///
/// This allows failed commands to remain accessible for `delay` more commands,
/// giving users a chance to use up-arrow to recall and fix typos.
#[derive(Debug, Default)]
pub struct SpongeQueue {
    /// Queue of command entries. Newer commands at front, older at back.
    queue: VecDeque<Option<HistoryItemId>>,
}

impl SpongeQueue {
    /// Create a new empty sponge queue.
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Record a command execution and return any history ID that should be deleted.
    ///
    /// - `failed`: whether the command failed
    /// - `history_id`: the history ID of the command (if available)
    /// - `delay`: how many commands to wait before deleting failed commands
    ///
    /// Returns `Some(id)` if an old failed command should be deleted from history.
    pub fn record_command(
        &mut self,
        failed: bool,
        history_id: Option<HistoryItemId>,
        delay: usize,
    ) -> Option<HistoryItemId> {
        // Add entry: Some(id) for failure, None for success
        let entry = if failed { history_id } else { None };
        self.queue.push_front(entry);

        // Check if we need to purge the oldest entry
        if self.queue.len() > delay
            && let Some(old_entry) = self.queue.pop_back()
        {
            // Return the ID if it was a failed command
            return old_entry;
        }

        None
    }

    /// Drain all remaining failed command IDs from the queue.
    ///
    /// Used during cleanup (e.g., on exit) to delete all tracked failed commands.
    pub fn drain_failed_ids(&mut self) -> impl Iterator<Item = HistoryItemId> + '_ {
        std::iter::from_fn(move || {
            while let Some(entry) = self.queue.pop_back() {
                if let Some(id) = entry {
                    return Some(id);
                }
            }
            None
        })
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}
