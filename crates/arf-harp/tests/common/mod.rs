//! Shared test helpers for arf-harp integration tests.

use once_cell::sync::OnceCell;
use std::sync::Mutex;

static R_LOCK: OnceCell<Mutex<()>> = OnceCell::new();

pub fn ensure_r_initialized() -> bool {
    static R_INITIALIZED: OnceCell<bool> = OnceCell::new();

    *R_INITIALIZED.get_or_init(|| unsafe {
        match arf_libr::initialize_r() {
            Ok(()) => true,
            Err(e) => {
                eprintln!("Failed to initialize R: {}", e);
                false
            }
        }
    })
}

/// Run a closure with the R runtime lock held.
///
/// Returns `None` if R initialization failed, `Some(result)` otherwise.
/// Uses poisoned-lock recovery so a panicking test does not block subsequent tests.
pub fn with_r<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> T,
{
    if !ensure_r_initialized() {
        return None;
    }
    let lock = R_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    Some(f())
}

/// Check that `LD_LIBRARY_PATH` includes the R library directory.
///
/// Tests that require package loading (e.g. utils, methods) must be skipped
/// when this is not set, because `initialize_r()` cannot re-exec the process
/// as the binary does via `ensure_ld_library_path()`.
pub fn ld_library_path_is_set() -> bool {
    let Ok(lib_path) = arf_libr::find_r_library() else {
        return false;
    };
    let Some(lib_dir) = lib_path.parent() else {
        return false;
    };
    let lib_dir_str = lib_dir.to_string_lossy();
    let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    current.split(':').any(|p| p == lib_dir_str.as_ref())
}
