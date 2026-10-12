//! Terminal width synchronization with R.

use crossterm::terminal;
use std::sync::atomic::{AtomicU16, Ordering};

/// Last known terminal width for detecting resize.
/// Updated by `sync_r_width()` to avoid redundant R calls.
static LAST_TERMINAL_WIDTH: AtomicU16 = AtomicU16::new(0);

/// Minimum width for R's `options(width)`, matching radian's behavior.
const MIN_R_WIDTH: u16 = 20;

/// Maximum width for R's `options(width)`. R enforces a hard maximum of 10000.
const MAX_R_WIDTH: u16 = 10000;

/// Sync R's `options(width)` with the current terminal width.
///
/// Compares the current terminal columns against the last known width.
/// If changed, updates R's width option. Called both at startup and
/// periodically from the idle callback to handle terminal resize.
pub(super) fn sync_r_width() {
    let prev = LAST_TERMINAL_WIDTH.load(Ordering::Relaxed);

    let (cols, _) = match terminal::size() {
        Ok(size) => size,
        Err(e) => {
            if prev != 0 {
                // Already have a known width; treat as transient failure.
                log::debug!(
                    "Failed to read terminal size (transient); keeping previous width: {:?}",
                    e
                );
                return;
            }
            // No previous width recorded; fall back to a reasonable default.
            log::debug!(
                "Failed to read terminal size; falling back to default width: {:?}",
                e
            );
            (80, 24)
        }
    };

    let clamped = cols.clamp(MIN_R_WIDTH, MAX_R_WIDTH);
    if prev != clamped {
        let code = format!("options(width = {})", clamped);
        match arf_harp::eval_string_with_visibility(&code) {
            Ok(_) => {
                LAST_TERMINAL_WIDTH.store(clamped, Ordering::Relaxed);
            }
            Err(e) => log::debug!("Failed to set R width option: {:?}", e),
        }
    }
}
