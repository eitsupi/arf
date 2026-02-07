//! Shared test utilities.
//!
//! This module provides helpers for tests that need to coordinate
//! access to process-global state like `current_dir`.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// Process-global mutex for tests that modify the current working directory.
///
/// `std::env::set_current_dir()` affects the entire process, so tests that
/// change cwd must hold this lock to avoid interfering with each other
/// during parallel test execution.
static CWD_MUTEX: Mutex<()> = Mutex::new(());

/// Acquire the cwd lock and save the current directory.
///
/// Returns a guard that restores the original directory on drop.
/// Tests that call `set_current_dir` should use this instead of
/// manually saving/restoring:
///
/// ```ignore
/// let _guard = test_utils::lock_cwd();
/// std::env::set_current_dir(tmp.path()).unwrap();
/// // ... test logic ...
/// // cwd is automatically restored when _guard drops
/// ```
pub fn lock_cwd() -> CwdGuard {
    let lock = CWD_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let original = std::env::current_dir().expect("failed to get current dir");
    CwdGuard {
        _lock: lock,
        original,
    }
}

/// RAII guard that holds the cwd mutex and restores the original directory on drop.
pub struct CwdGuard {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}
