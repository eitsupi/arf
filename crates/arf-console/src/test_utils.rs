//! Shared test utilities.
//!
//! This module provides helpers for tests that need to coordinate
//! access to process-global state like `current_dir`.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// Process-global mutex for tests that modify the current working directory.
///
/// `std::env::set_current_dir()` affects the entire process, so tests that
/// change cwd must hold this lock to avoid interfering with each other
/// during parallel test execution.
static CWD_MUTEX: Mutex<()> = Mutex::new(());

/// Process-global mutex for tests that modify environment variables.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

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
        if let Err(err) = std::env::set_current_dir(&self.original) {
            if std::thread::panicking() {
                eprintln!(
                    "CwdGuard: failed to restore original working directory {:?}: {}",
                    self.original, err
                );
            } else {
                panic!(
                    "CwdGuard: failed to restore original working directory {:?}: {}",
                    self.original, err
                );
            }
        }
    }
}

/// Set an environment variable while holding a lock and restore its original
/// value when the returned guard is dropped.
pub fn lock_env_var(name: &'static str, value: impl AsRef<OsStr>) -> EnvVarGuard {
    let lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let original = std::env::var_os(name);
    // SAFETY: Tests serialize access to these process-global variables.
    unsafe { std::env::set_var(name, value) };
    EnvVarGuard {
        _lock: lock,
        name,
        original,
    }
}

/// RAII guard that holds the environment-variable mutex and restores the
/// original value on drop.
pub struct EnvVarGuard {
    _lock: MutexGuard<'static, ()>,
    name: &'static str,
    original: Option<OsString>,
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: Tests serialize access to these process-global variables.
        unsafe {
            if let Some(value) = &self.original {
                std::env::set_var(self.name, value);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }
}
