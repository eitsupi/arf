//! Integration tests for R code completion.

mod common;

use arf_harp::completion::get_completions;
use arf_harp::eval_string_with_visibility;
use common::{ld_library_path_is_set, with_r};

/// Regression test for GitHub issue #204:
/// Tab completion should work inside function call arguments.
///
/// R's `.completeToken()` takes significantly longer (~150ms) when inside a
/// function call than at the top level (~20ms), because it also looks up
/// function argument names. The 50ms default timeout causes these completions
/// to time out and return empty.
#[test]
fn test_completion_inside_function_call() {
    if !ld_library_path_is_set() {
        eprintln!(
            "Skipping test_completion_inside_function_call: \
             LD_LIBRARY_PATH not set."
        );
        return;
    }

    with_r(|| {
        eval_string_with_visibility("aaa_bbb <- 1").expect("assignment should succeed");

        // timeout_ms=1 forces the timeout path on any platform, making this a genuine
        // red test before the fix. The fix bypasses timeout when inside a function call.
        let completions = get_completions("str(aaa_", 8, 1).expect("should not error");

        assert!(
            completions.iter().any(|c| c == "aaa_bbb"),
            "Expected 'aaa_bbb' in completions for 'str(aaa_' at pos 8 \
             (timeout_ms=1, bypassed inside function call), got: {:?}",
            completions
        );
    });
}

/// Baseline: top-level completion finishes well within 50ms.
#[test]
fn test_completion_at_top_level_within_timeout() {
    if !ld_library_path_is_set() {
        eprintln!(
            "Skipping test_completion_at_top_level_within_timeout: \
             LD_LIBRARY_PATH not set."
        );
        return;
    }

    with_r(|| {
        eval_string_with_visibility("aaa_bbb <- 1").expect("assignment should succeed");

        let completions = get_completions("aaa_", 4, 50).expect("should not error");

        assert!(
            completions.iter().any(|c| c == "aaa_bbb"),
            "Expected 'aaa_bbb' in completions for 'aaa_' at pos 4, got: {:?}",
            completions
        );
    });
}
