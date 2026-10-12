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
fn test_value_expressions_are_visible() {
    with_r!(test_value_expressions_are_visible, {
        eval_string_with_visibility("test_var <- 123").expect("assignment should succeed");

        for code in [
            "1 + 1",
            "test_var",
            "sum(1, 2, 3)",
            r#""hello""#,
            "c(1, 2, 3)",
        ] {
            let result = eval_string_with_visibility(code).expect("eval should succeed");
            assert!(result.visible, "`{code}` should be visible");
        }
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
