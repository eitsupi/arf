use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Holds the process-global environment lock and restores `R_DOC_DIR` on drop.
///
/// This guard is intentionally duplicated locally instead of being extracted
/// into a shared crate: Rust builds each crate's tests into a separate binary,
/// so a static mutex in a shared crate would not become one lock across crates.
/// Every test binary links its own copy, so sharing would provide code reuse
/// only and no additional safety. With one protected test in arf-libr, a new
/// workspace member is not worth the manifest, ownership, and maintenance
/// overhead. Revisit extraction once a third crate needs these guards, or once
/// arf-libr grows several tests needing multi-variable or CWD handling.
pub(crate) struct RDocDirGuard {
    _lock: MutexGuard<'static, ()>,
    original: Option<OsString>,
}

impl RDocDirGuard {
    pub(crate) fn new(value: &str) -> Self {
        let lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var_os("R_DOC_DIR");

        // SAFETY: Tests hold ENV_MUTEX while mutating this process-global variable.
        unsafe { std::env::set_var("R_DOC_DIR", value) };

        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for RDocDirGuard {
    fn drop(&mut self) {
        // SAFETY: Tests hold ENV_MUTEX while restoring this process-global variable.
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var("R_DOC_DIR", value),
                None => std::env::remove_var("R_DOC_DIR"),
            }
        }
    }
}
