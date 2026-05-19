//! Integration tests for R help rendering via rd2qmd.

use arf_harp::get_help_markdown;
use once_cell::sync::OnceCell;
use std::sync::Mutex;

static R_LOCK: OnceCell<Mutex<()>> = OnceCell::new();

fn ensure_r_initialized() -> bool {
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

fn with_r<F, T>(f: F) -> Option<T>
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

fn ld_library_path_is_set() -> bool {
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

/// Regression test for GitHub issue #194:
/// `base::solve` contains `%*%` in its Rd source. Without `deparse = TRUE`,
/// `as.character()` emits unescaped `%` which rd-parser treats as a comment,
/// losing closing braces and producing a parse error. With `deparse = TRUE`
/// the `%` is escaped as `\%` and rd2qmd parses the page correctly.
#[cfg(target_os = "linux")]
#[test]
fn test_help_base_solve_returns_content() {
    if !ld_library_path_is_set() {
        eprintln!(
            "Skipping test_help_base_solve_returns_content: \
             LD_LIBRARY_PATH not set."
        );
        return;
    }

    let Some(()) = with_r(|| {
        let result = get_help_markdown("solve", Some("base"));
        match &result {
            Err(e) => panic!(r#"get_help_markdown("solve", Some("base")) failed: {e}"#),
            Ok(md) => {
                assert!(!md.is_empty(), "help markdown must not be empty");
                // The title of the help page is "Solve a System of Equations"
                assert!(
                    md.contains("Solve"),
                    "expected 'Solve' in help markdown, got:\n{md}"
                );
                // The usage section must contain ellipses (from \dots) and braces
                // from R examples — regression check for rd-parser fixes in PR #21.
                assert!(
                    md.contains("...") || md.contains('\u{2026}'),
                    "expected ellipses in help markdown (\\dots regression), got:\n{md}"
                );
            }
        }
    }) else {
        panic!("R initialization failed; cannot run test_help_base_solve_returns_content");
    };
}
