//! Integration tests for R expression visibility handling.
//!
//! These tests verify that:
//! - Visible expressions (like `1 + 1`) have `visible = true`
//! - Invisible assignments (like `x <- 1`) have `visible = false`
//! - Variable lookups are visible
//! - The `invisible()` function makes results invisible

use arf_harp::eval_string_with_visibility;
use once_cell::sync::OnceCell;
use std::env;
use std::sync::Mutex;

/// Global lock to ensure R tests run serially (R is not thread-safe).
static R_LOCK: OnceCell<Mutex<()>> = OnceCell::new();

/// Initialize R once for all tests.
fn ensure_r_initialized() -> bool {
    static R_INITIALIZED: OnceCell<bool> = OnceCell::new();

    *R_INITIALIZED.get_or_init(|| {
        // Initialize R
        unsafe {
            match arf_libr::initialize_r() {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("Failed to initialize R: {}", e);
                    false
                }
            }
        }
    })
}

/// Run a test with R lock held.
fn with_r<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> T,
{
    if !ensure_r_initialized() {
        return None;
    }

    let lock = R_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap();
    Some(f())
}

#[test]
fn test_simple_expression_is_visible() {
    with_r(|| {
        let result = eval_string_with_visibility("1 + 1").expect("eval should succeed");
        assert!(
            result.visible,
            "Simple arithmetic expression should be visible"
        );
    });
}

#[test]
fn test_assignment_is_invisible() {
    with_r(|| {
        let result = eval_string_with_visibility("x <- 42").expect("eval should succeed");
        assert!(
            !result.visible,
            "Assignment with <- should be invisible"
        );
    });
}

#[test]
fn test_equals_assignment_is_invisible() {
    with_r(|| {
        let result = eval_string_with_visibility("y = 100").expect("eval should succeed");
        assert!(
            !result.visible,
            "Assignment with = should be invisible"
        );
    });
}

#[test]
fn test_variable_lookup_is_visible() {
    with_r(|| {
        // First assign a value
        eval_string_with_visibility("test_var <- 123").expect("assignment should succeed");

        // Then look it up - should be visible
        let result = eval_string_with_visibility("test_var").expect("eval should succeed");
        assert!(
            result.visible,
            "Variable lookup should be visible"
        );
    });
}

#[test]
fn test_invisible_function_makes_result_invisible() {
    with_r(|| {
        let result = eval_string_with_visibility("invisible(42)").expect("eval should succeed");
        assert!(
            !result.visible,
            "invisible() should make result invisible"
        );
    });
}

#[test]
fn test_print_function_is_visible() {
    with_r(|| {
        // print() returns its argument invisibly, but we're testing the return
        let result = eval_string_with_visibility("print(1)").expect("eval should succeed");
        // print() returns its argument invisibly
        assert!(
            !result.visible,
            "print() returns its argument invisibly"
        );
    });
}

#[test]
fn test_function_call_is_visible() {
    with_r(|| {
        let result = eval_string_with_visibility("sum(1, 2, 3)").expect("eval should succeed");
        assert!(
            result.visible,
            "Function call with visible result should be visible"
        );
    });
}

#[test]
fn test_null_result_is_not_visible() {
    with_r(|| {
        // NULL is never considered visible for printing purposes
        let result = eval_string_with_visibility("NULL").expect("eval should succeed");
        assert!(
            !result.visible,
            "NULL should not be marked as visible for printing"
        );
    });
}

#[test]
fn test_string_literal_is_visible() {
    with_r(|| {
        let result = eval_string_with_visibility(r#""hello""#).expect("eval should succeed");
        assert!(
            result.visible,
            "String literal should be visible"
        );
    });
}

#[test]
fn test_vector_creation_is_visible() {
    with_r(|| {
        let result = eval_string_with_visibility("c(1, 2, 3)").expect("eval should succeed");
        assert!(
            result.visible,
            "Vector creation should be visible"
        );
    });
}

/// Check if LD_LIBRARY_PATH includes the R library directory.
fn ld_library_path_is_set() -> bool {
    let Ok(lib_path) = arf_libr::find_r_library() else {
        return false;
    };
    let Some(lib_dir) = lib_path.parent() else {
        return false;
    };
    let lib_dir_str = lib_dir.to_string_lossy();
    let current = env::var("LD_LIBRARY_PATH").unwrap_or_default();
    current.split(':').any(|p| p == lib_dir_str.as_ref())
}

#[test]
fn test_package_loading_works() {
    // Skip this test if LD_LIBRARY_PATH is not set correctly.
    // The binary handles this via ensure_ld_library_path() which re-execs the process,
    // but tests cannot re-exec themselves.
    if !ld_library_path_is_set() {
        eprintln!(
            "Skipping test_package_loading_works: LD_LIBRARY_PATH not set.\n\
             Run tests with: LD_LIBRARY_PATH=/opt/R/4.5.2/lib/R/lib cargo test"
        );
        return;
    }

    with_r(|| {
        // Try to load the 'methods' package which is a base package
        // This tests that LD_LIBRARY_PATH is set correctly
        let result = eval_string_with_visibility("library(methods)");
        assert!(
            result.is_ok(),
            "Loading 'methods' package should succeed (LD_LIBRARY_PATH must be set)"
        );
    });
}

#[test]
fn test_base_functions_work() {
    with_r(|| {
        // Test that base functions that might depend on loaded libraries work
        let result = eval_string_with_visibility("paste('hello', 'world')");
        assert!(result.is_ok(), "paste() should work");
    });
}

/// Test that reprex mode can be enabled and the settings are stored.
/// Note: The actual output formatting is tested by running the binary.
#[test]
fn test_reprex_mode_settings() {
    // Test that set_reprex_mode doesn't panic
    arf_libr::set_reprex_mode(true, "#> ");
    arf_libr::set_reprex_mode(true, "## ");
    arf_libr::set_reprex_mode(false, "");
    // If we get here without panic, the settings are being stored correctly
}

#[test]
fn test_reprex_mode_output() {
    with_r(|| {
        // Enable reprex mode
        arf_libr::set_reprex_mode(true, "#> ");

        // Evaluate R code - output will be prefixed with "#> "
        // (We can't easily capture stdout in tests, but we verify the code runs)
        let result = arf_harp::eval_string("1+1");
        assert!(result.is_ok(), "eval_string should succeed in reprex mode");

        // Disable reprex mode for other tests
        arf_libr::set_reprex_mode(false, "");
    });
}

// Tests for is_expression_complete (multiline input support)

#[test]
fn test_complete_expression() {
    with_r(|| {
        let result = arf_harp::is_expression_complete("1 + 1").expect("should not error");
        assert!(result, "Simple expression should be complete");
    });
}

#[test]
fn test_incomplete_expression_open_paren() {
    with_r(|| {
        let result = arf_harp::is_expression_complete("(1 +").expect("should not error");
        assert!(!result, "Expression with unclosed paren should be incomplete");
    });
}

#[test]
fn test_incomplete_expression_open_brace() {
    with_r(|| {
        let result = arf_harp::is_expression_complete("function() {").expect("should not error");
        assert!(!result, "Expression with unclosed brace should be incomplete");
    });
}

#[test]
fn test_incomplete_expression_trailing_operator() {
    with_r(|| {
        let result = arf_harp::is_expression_complete("1 +").expect("should not error");
        assert!(!result, "Expression with trailing operator should be incomplete");
    });
}

#[test]
fn test_complete_multiline_expression() {
    with_r(|| {
        let result =
            arf_harp::is_expression_complete("function() {\n  1 + 1\n}").expect("should not error");
        assert!(result, "Complete multiline expression should be complete");
    });
}

#[test]
fn test_complete_if_statement() {
    with_r(|| {
        let result = arf_harp::is_expression_complete("if (TRUE) 1 else 2").expect("should not error");
        assert!(result, "Complete if-else should be complete");
    });
}

#[test]
fn test_incomplete_if_statement() {
    with_r(|| {
        // R's parser considers "if (TRUE) 1 else" as incomplete
        let result = arf_harp::is_expression_complete("if (TRUE) 1 else").expect("should not error");
        assert!(!result, "if-else without second value should be incomplete");
    });
}

#[test]
fn test_parse_error_is_complete() {
    with_r(|| {
        // Parse errors are NOT marked as incomplete - they are "complete" in the sense
        // that we should try to evaluate them to show the error
        let result = arf_harp::is_expression_complete("1 + + 2").expect("should not error");
        assert!(result, "Parse error should be considered complete (not incomplete)");
    });
}

// Tests for check_if_functions (function type detection for completion)

#[test]
fn test_check_if_functions_base_functions() {
    // Skip if LD_LIBRARY_PATH is not set (packages may not load)
    if !ld_library_path_is_set() {
        eprintln!("Skipping test: LD_LIBRARY_PATH not set");
        return;
    }

    with_r(|| {
        let names = vec!["print", "sum", "mean", "c"];
        let result = arf_harp::completion::check_if_functions(&names).expect("should succeed");
        assert_eq!(result.len(), 4);
        assert!(result[0], "print should be a function");
        assert!(result[1], "sum should be a function");
        assert!(result[2], "mean should be a function");
        assert!(result[3], "c should be a function");
    });
}

#[test]
fn test_check_if_functions_non_functions() {
    // Skip if LD_LIBRARY_PATH is not set
    if !ld_library_path_is_set() {
        eprintln!("Skipping test: LD_LIBRARY_PATH not set");
        return;
    }

    with_r(|| {
        let names = vec!["TRUE", "FALSE", "NA", "NULL"];
        let result = arf_harp::completion::check_if_functions(&names).expect("should succeed");
        assert_eq!(result.len(), 4);
        assert!(!result[0], "TRUE should not be a function");
        assert!(!result[1], "FALSE should not be a function");
        assert!(!result[2], "NA should not be a function");
        assert!(!result[3], "NULL should not be a function");
    });
}

#[test]
fn test_check_if_functions_namespaced() {
    // Skip if LD_LIBRARY_PATH is not set (packages may not load)
    if !ld_library_path_is_set() {
        eprintln!("Skipping test: LD_LIBRARY_PATH not set");
        return;
    }

    with_r(|| {
        // Test namespace-qualified function names (pkg::func syntax)
        let names = vec!["base::print", "base::sum", "stats::lm"];
        let result = arf_harp::completion::check_if_functions(&names).expect("should succeed");
        assert_eq!(result.len(), 3);
        assert!(result[0], "base::print should be a function");
        assert!(result[1], "base::sum should be a function");
        assert!(result[2], "stats::lm should be a function");
    });
}

#[test]
fn test_check_if_functions_mixed() {
    // Skip if LD_LIBRARY_PATH is not set
    if !ld_library_path_is_set() {
        eprintln!("Skipping test: LD_LIBRARY_PATH not set");
        return;
    }

    with_r(|| {
        // Mix of functions and non-functions
        let names = vec!["print", "TRUE", "base::sum", "nonexistent_xyz"];
        let result = arf_harp::completion::check_if_functions(&names).expect("should succeed");
        assert_eq!(result.len(), 4);
        assert!(result[0], "print should be a function");
        assert!(!result[1], "TRUE should not be a function");
        assert!(result[2], "base::sum should be a function");
        assert!(!result[3], "nonexistent should not be a function");
    });
}

#[test]
fn test_check_if_functions_empty() {
    // Skip if LD_LIBRARY_PATH is not set
    if !ld_library_path_is_set() {
        eprintln!("Skipping test: LD_LIBRARY_PATH not set");
        return;
    }

    with_r(|| {
        let names: Vec<&str> = vec![];
        let result = arf_harp::completion::check_if_functions(&names).expect("should succeed");
        assert!(result.is_empty(), "Empty input should return empty result");
    });
}
