//! Small synchronization helpers shared across the crate.

use std::sync::{Mutex, MutexGuard};

/// Extension for locking a [`Mutex`] while ignoring poisoning.
pub(crate) trait LockRecover<T> {
    /// Lock the mutex, recovering the guard if a previous holder panicked.
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
