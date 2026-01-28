//! SEXP protection mechanism using RAII.
//!
//! R uses a protection stack to prevent garbage collection of objects.
//! This module provides a RAII wrapper to ensure objects are properly
//! protected and unprotected.

use arf_libr::{r_library, SEXP};

/// RAII guard for R's protection stack.
///
/// When created, it protects the given SEXP. When dropped, it unprotects it.
#[derive(Debug)]
pub struct RProtect {
    count: i32,
}

impl RProtect {
    /// Create a new protection guard with zero protected objects.
    pub fn new() -> Self {
        RProtect { count: 0 }
    }

    /// Protect a SEXP and increment the protection count.
    ///
    /// # Safety
    /// The caller must ensure that `sexp` is a valid R object.
    pub unsafe fn protect(&mut self, sexp: SEXP) -> SEXP {
        if let Ok(lib) = r_library() {
            // SAFETY: The caller guarantees sexp is valid
            let protected = unsafe { (lib.rf_protect)(sexp) };
            self.count += 1;
            protected
        } else {
            sexp
        }
    }

    /// Get the current protection count.
    pub fn count(&self) -> i32 {
        self.count
    }
}

impl Default for RProtect {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RProtect {
    fn drop(&mut self) {
        if self.count > 0 {
            if let Ok(lib) = r_library() {
                unsafe {
                    (lib.rf_unprotect)(self.count);
                }
            }
        }
    }
}

/// Protect a single SEXP for the duration of a scope.
///
/// # Safety
/// The caller must ensure that `sexp` is a valid R object.
pub unsafe fn with_protected<F, R>(sexp: SEXP, f: F) -> R
where
    F: FnOnce(SEXP) -> R,
{
    let mut protect = RProtect::new();
    // SAFETY: The caller guarantees sexp is valid
    let protected = unsafe { protect.protect(sexp) };
    f(protected)
}
