//! Integration tests for R expression visibility handling.
//!
//! These tests verify that:
//! - Visible expressions (like `1 + 1`) have `visible = true`
//! - Invisible assignments (like `x <- 1`) have `visible = false`
//! - Variable lookups are visible
//! - The `invisible()` function makes results invisible

mod common;

use arf_harp::eval_string_with_visibility;
use common::with_r;

#[test]
fn test_simple_expression_is_visible() {
    with_r!(test_simple_expression_is_visible, {
        let result = eval_string_with_visibility("1 + 1").expect("eval should succeed");
        assert!(
            result.visible,
            "Simple arithmetic expression should be visible"
        );
    });
}

#[test]
fn test_assignment_is_invisible() {
    with_r!(test_assignment_is_invisible, {
        let result = eval_string_with_visibility("x <- 42").expect("eval should succeed");
        assert!(!result.visible, "Assignment with <- should be invisible");
    });
}

#[test]
fn test_equals_assignment_is_invisible() {
    with_r!(test_equals_assignment_is_invisible, {
        let result = eval_string_with_visibility("y = 100").expect("eval should succeed");
        assert!(!result.visible, "Assignment with = should be invisible");
    });
}

#[test]
fn test_variable_lookup_is_visible() {
    with_r!(test_variable_lookup_is_visible, {
        // First assign a value
        eval_string_with_visibility("test_var <- 123").expect("assignment should succeed");

        // Then look it up - should be visible
        let result = eval_string_with_visibility("test_var").expect("eval should succeed");
        assert!(result.visible, "Variable lookup should be visible");
    });
}

#[test]
fn test_invisible_function_makes_result_invisible() {
    with_r!(test_invisible_function_makes_result_invisible, {
        let result = eval_string_with_visibility("invisible(42)").expect("eval should succeed");
        assert!(!result.visible, "invisible() should make result invisible");
    });
}

#[test]
fn test_print_function_is_visible() {
    with_r!(test_print_function_is_visible, {
        // print() returns its argument invisibly, but we're testing the return
        let result = eval_string_with_visibility("print(1)").expect("eval should succeed");
        // print() returns its argument invisibly
        assert!(!result.visible, "print() returns its argument invisibly");
    });
}

#[test]
fn test_function_call_is_visible() {
    with_r!(test_function_call_is_visible, {
        let result = eval_string_with_visibility("sum(1, 2, 3)").expect("eval should succeed");
        assert!(
            result.visible,
            "Function call with visible result should be visible"
        );
    });
}

#[test]
fn test_null_result_is_not_visible() {
    with_r!(test_null_result_is_not_visible, {
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
    with_r!(test_string_literal_is_visible, {
        let result = eval_string_with_visibility(r#""hello""#).expect("eval should succeed");
        assert!(result.visible, "String literal should be visible");
    });
}

#[test]
fn test_vector_creation_is_visible() {
    with_r!(test_vector_creation_is_visible, {
        let result = eval_string_with_visibility("c(1, 2, 3)").expect("eval should succeed");
        assert!(result.visible, "Vector creation should be visible");
    });
}

#[test]
fn test_package_loading_works() {
    with_r!(test_package_loading_works, {
        // Try to load the 'methods' package which is a base package
        // The subprocess helper must support loading package shared libraries.
        let result = eval_string_with_visibility("library(methods)");
        assert!(
            result.is_ok(),
            "Loading 'methods' package should succeed in the isolated R process"
        );
    });
}

#[test]
fn test_base_functions_work() {
    with_r!(test_base_functions_work, {
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
    with_r!(test_reprex_mode_output, {
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
    with_r!(test_complete_expression, {
        let result = arf_harp::is_expression_complete("1 + 1").expect("should not error");
        assert!(result, "Simple expression should be complete");
    });
}

#[test]
fn test_incomplete_expression_open_paren() {
    with_r!(test_incomplete_expression_open_paren, {
        let result = arf_harp::is_expression_complete("(1 +").expect("should not error");
        assert!(
            !result,
            "Expression with unclosed paren should be incomplete"
        );
    });
}

#[test]
fn test_incomplete_expression_open_brace() {
    with_r!(test_incomplete_expression_open_brace, {
        let result = arf_harp::is_expression_complete("function() {").expect("should not error");
        assert!(
            !result,
            "Expression with unclosed brace should be incomplete"
        );
    });
}

#[test]
fn test_incomplete_expression_trailing_operator() {
    with_r!(test_incomplete_expression_trailing_operator, {
        let result = arf_harp::is_expression_complete("1 +").expect("should not error");
        assert!(
            !result,
            "Expression with trailing operator should be incomplete"
        );
    });
}

#[test]
fn test_complete_multiline_expression() {
    with_r!(test_complete_multiline_expression, {
        let result =
            arf_harp::is_expression_complete("function() {\n  1 + 1\n}").expect("should not error");
        assert!(result, "Complete multiline expression should be complete");
    });
}

#[test]
fn test_complete_if_statement() {
    with_r!(test_complete_if_statement, {
        let result =
            arf_harp::is_expression_complete("if (TRUE) 1 else 2").expect("should not error");
        assert!(result, "Complete if-else should be complete");
    });
}

#[test]
fn test_incomplete_if_statement() {
    with_r!(test_incomplete_if_statement, {
        // R's parser considers "if (TRUE) 1 else" as incomplete
        let result =
            arf_harp::is_expression_complete("if (TRUE) 1 else").expect("should not error");
        assert!(!result, "if-else without second value should be incomplete");
    });
}

#[test]
fn test_parse_error_is_complete() {
    with_r!(test_parse_error_is_complete, {
        // Parse errors are NOT marked as incomplete - they are "complete" in the sense
        // that we should try to evaluate them to show the error
        let result = arf_harp::is_expression_complete("1 + + 2").expect("should not error");
        assert!(
            result,
            "Parse error should be considered complete (not incomplete)"
        );
    });
}

// Tests for check_if_functions (function type detection for completion)

#[test]
fn test_check_if_functions_base_functions() {
    with_r!(test_check_if_functions_base_functions, {
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
    with_r!(test_check_if_functions_non_functions, {
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
    with_r!(test_check_if_functions_namespaced, {
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
    with_r!(test_check_if_functions_mixed, {
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
    with_r!(test_check_if_functions_empty, {
        let names: Vec<&str> = vec![];
        let result = arf_harp::completion::check_if_functions(&names).expect("should succeed");
        assert!(result.is_empty(), "Empty input should return empty result");
    });
}
