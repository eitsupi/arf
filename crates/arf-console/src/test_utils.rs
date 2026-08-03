//! Shared test utilities.
//!
//! This module provides helpers for tests that need to coordinate
//! access to process-global state like `current_dir`.

use std::collections::BTreeMap;
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

/// Acquire the environment-variable lock.
///
/// Hold one `EnvGuard` for the duration of a test and mutate all required
/// variables through it.
pub fn lock_env() -> EnvGuard {
    let lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    EnvGuard {
        _lock: lock,
        originals: BTreeMap::new(),
    }
}

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

/// RAII guard that holds the environment-variable mutex and restores every
/// variable changed through this guard when it is dropped.
///
/// Variable names are compared byte for byte, so on Windows `PATH` and `Path`
/// are tracked as two separate variables even though the OS treats them as
/// one. Use a single spelling per variable within one test.
pub struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    originals: BTreeMap<OsString, Option<OsString>>,
}

impl EnvGuard {
    /// Set an environment variable, saving its original value if this is the
    /// first mutation of the variable through this guard.
    pub fn set(&mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        let name = name.as_ref().to_os_string();
        self.originals
            .entry(name.clone())
            .or_insert_with(|| std::env::var_os(&name));
        // SAFETY: Tests serialize access to these process-global variables.
        unsafe { std::env::set_var(&name, value) };
    }

    /// Remove an environment variable, saving its original value if this is
    /// the first mutation of the variable through this guard.
    pub fn unset(&mut self, name: impl AsRef<OsStr>) {
        let name = name.as_ref().to_os_string();
        self.originals
            .entry(name.clone())
            .or_insert_with(|| std::env::var_os(&name));
        // SAFETY: Tests serialize access to these process-global variables.
        unsafe { std::env::remove_var(&name) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // BTreeMap keeps restoration order deterministic.
        // SAFETY: Tests serialize access to these process-global variables.
        unsafe {
            for (name, original) in &self.originals {
                if let Some(value) = original {
                    std::env::set_var(name, value);
                } else {
                    std::env::remove_var(name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_env_restores_multiple_variables() {
        let first = "ARF_TEST_UTILS_MULTIPLE_FIRST";
        let second = "ARF_TEST_UTILS_MULTIPLE_SECOND";
        let original_first = std::env::var_os(first);
        let original_second = std::env::var_os(second);

        {
            let mut guard = lock_env();
            guard.set(first, "changed-first");
            guard.set(second, "changed-second");
            assert_eq!(
                std::env::var_os(first),
                Some(OsString::from("changed-first"))
            );
            assert_eq!(
                std::env::var_os(second),
                Some(OsString::from("changed-second"))
            );
        }

        assert_eq!(std::env::var_os(first), original_first);
        assert_eq!(std::env::var_os(second), original_second);
    }

    #[test]
    fn lock_env_restores_first_original_value_after_repeated_mutations() {
        let name = "ARF_TEST_UTILS_REPEATED";
        let original = std::env::var_os(name);

        {
            let mut guard = lock_env();
            guard.set(name, "first-change");
            guard.unset(name);
            guard.set(name, "second-change");
        }

        assert_eq!(std::env::var_os(name), original);
    }

    #[test]
    fn lock_env_supports_mixed_set_and_unset_mutations() {
        let set_name = "ARF_TEST_UTILS_MIXED_SET";
        let unset_name = "ARF_TEST_UTILS_MIXED_UNSET";
        let original_set = std::env::var_os(set_name);
        let original_unset = std::env::var_os(unset_name);

        {
            let mut guard = lock_env();
            guard.set(set_name, "set-value");
            guard.unset(unset_name);
            assert_eq!(
                std::env::var_os(set_name),
                Some(OsString::from("set-value"))
            );
            assert_eq!(std::env::var_os(unset_name), None);
        }

        assert_eq!(std::env::var_os(set_name), original_set);
        assert_eq!(std::env::var_os(unset_name), original_unset);
    }

    #[test]
    fn lock_env_restores_a_variable_that_was_originally_unset() {
        // This name is used by no other test and by no production code, so it
        // is guaranteed to start out unset. Asserting that while holding the
        // guard avoids the race a lock-then-unlock preamble would introduce.
        let name = "ARF_TEST_UTILS_ORIGINALLY_UNSET";

        {
            let mut guard = lock_env();
            assert_eq!(std::env::var_os(name), None);
            guard.set(name, "temporary-value");
        }

        assert_eq!(std::env::var_os(name), None);
    }

    #[test]
    fn lock_env_allows_mutating_two_variables_without_deadlocking() {
        let first = "ARF_TEST_UTILS_NO_DEADLOCK_FIRST";
        let second = "ARF_TEST_UTILS_NO_DEADLOCK_SECOND";
        let original_first = std::env::var_os(first);
        let original_second = std::env::var_os(second);

        {
            let mut guard = lock_env();
            guard.set(first, "first-value");
            guard.set(second, "second-value");
        }

        assert_eq!(std::env::var_os(first), original_first);
        assert_eq!(std::env::var_os(second), original_second);
    }
}
