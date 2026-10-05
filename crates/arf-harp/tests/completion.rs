//! Integration tests for R code completion.

mod common;

use arf_harp::completion::get_completions;
use arf_harp::completion::get_installed_packages;
use arf_harp::eval_string_with_visibility;
use common::with_r;

/// Regression test for GitHub issue #204:
/// Tab completion should work inside function call arguments.
///
/// R's `.completeToken()` takes significantly longer (~150ms) when inside a
/// function call than at the top level (~20ms), because it also looks up
/// function argument names. The 50ms default timeout causes these completions
/// to time out and return empty.
#[test]
fn test_completion_inside_function_call() {
    with_r!(test_completion_inside_function_call, {
        eval_string_with_visibility("aaa_bbb <- 1").expect("assignment should succeed");

        // timeout_ms=1 would cause a timeout at top level, but inside a function call
        // the effective timeout is raised to 1000ms so the completion succeeds.
        let completions = get_completions("str(aaa_", 8, 1).expect("should not error");

        assert!(
            completions.iter().any(|c| c == "aaa_bbb"),
            "Expected 'aaa_bbb' in completions for 'str(aaa_' at pos 8 \
             (timeout_ms=1, raised to 1000ms inside function call), got: {:?}",
            completions
        );
    });
}

/// Baseline: top-level completion finishes well within 50ms.
#[test]
fn test_completion_at_top_level_within_timeout() {
    with_r!(test_completion_at_top_level_within_timeout, {
        eval_string_with_visibility("aaa_bbb <- 1").expect("assignment should succeed");

        let completions = get_completions("aaa_", 4, 50).expect("should not error");

        assert!(
            completions.iter().any(|c| c == "aaa_bbb"),
            "Expected 'aaa_bbb' in completions for 'aaa_' at pos 4, got: {:?}",
            completions
        );
    });
}

/// Observe the time limits passed to R without relying on machine speed.
#[test]
fn test_completion_timeout_policy_across_contexts() {
    with_r!(test_completion_timeout_policy_across_contexts, {
        eval_string_with_visibility(
            r#"
            aaa_bbb <- 1
            trace("setTimeLimit", where = baseenv(), print = FALSE,
                  tracer = quote({
                      .GlobalEnv$.completion_limits <- c(
                          .GlobalEnv$.completion_limits, cpu, elapsed)
                  }))
            "#,
        )
        .expect("time-limit observation should be installed");

        for (line, cursor, requested, expected) in [
            ("str(aaa_", 8, 1, 1000),    // Also offers package namespace candidates.
            ("str(aaa_)", 8, 1, 1000),   // Auto-inserted closing parenthesis.
            ("str(", 4, 1, 1000),        // No identifier/package candidate context.
            ("stats::lm", 9, 1, 1000),   // Namespace operator.
            ("aaa_", 4, 50, 50),         // Top-level budget is unchanged.
            ("str(aaa_", 8, 2000, 2000), // The floor does not cap larger budgets.
            ("str(aaa_", 8, 0, 0),       // An explicit unlimited budget stays unlimited.
            ("str(", 4, 0, 0),
            ("stats::lm", 9, 0, 0),
            ("aaa_", 4, 0, 0),
        ] {
            eval_string_with_visibility(".completion_limits <- numeric(0)")
                .expect("observation should reset");
            get_completions(line, cursor, requested).expect("completion should not error");
            // A bounded request sets both limits, then clears them after completion.
            let assertion = if expected == 0 {
                "stopifnot(length(.completion_limits) == 0L)".to_owned()
            } else {
                let seconds = expected as f64 / 1000.0;
                format!(
                    "stopifnot(identical(.completion_limits, c({seconds}, {seconds}, Inf, Inf)))"
                )
            };
            eval_string_with_visibility(&assertion).unwrap_or_else(|error| {
                panic!(
                    "unexpected R time limits for {line:?} at {cursor}, \
                     requested={requested}ms, expected={expected}ms: {error}"
                )
            });
        }
    });
}

#[test]
fn test_installed_packages_include_base_packages() {
    with_r!(test_installed_packages_include_base_packages, {
        let packages = get_installed_packages().expect("package scan should not error");
        assert!(
            packages.iter().any(|package| package == "base"),
            "installed package scan should include base, got: {packages:?}"
        );
        assert!(
            packages.iter().any(|package| package == "utils"),
            "installed package scan should include utils, got: {packages:?}"
        );
    });
}
