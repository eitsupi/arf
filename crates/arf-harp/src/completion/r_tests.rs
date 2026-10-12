//! R-dependent completion measurements, isolated from other unit tests.
//!
//! Run with an installed R package corpus. The subprocess helper configures
//! the selected R library's runtime search path before launching each R test:
//! `cargo test -p arf-harp --lib completion::r_tests -- --ignored --nocapture`
//! No Cargo feature is required. Each test runs in a fresh child process.

use super::static_formals::{StaticFormalsResult, lookup_with_policy, production_candidates};
use super::{
    StaticFormalsMode, StaticFormalsPolicy, check_if_functions, get_completions_with_policy,
    get_r_completions, get_token,
};
use crate::ffi::r_evaluation_count;
use crate::lib_paths;
use crate::test_support::with_r_in_subprocess;

/// Look up static formals with static inspection enabled and no exclusions.
fn lookup(line: &str, cursor_pos: usize) -> Option<StaticFormalsResult> {
    let policy = StaticFormalsPolicy {
        mode: StaticFormalsMode::PreferStatic,
        ..StaticFormalsPolicy::off()
    };
    lookup_with_policy(line, cursor_pos, &policy)
}

#[test]
#[ignore = "requires an installed R package corpus"]
fn experimental_static_first_hit_and_fallback() {
    with_r_in_subprocess(
        concat!(
            module_path!(),
            "::experimental_static_first_hit_and_fallback"
        ),
        || {
            lib_paths::populate_lib_paths().expect("R library paths should be available");
            let policy = StaticFormalsPolicy {
                mode: StaticFormalsMode::PreferStatic,
                ..StaticFormalsPolicy::default()
            };
            let line = "stats::lm(";
            let off_before = r_evaluation_count();
            let _ = get_completions_with_policy(
                line,
                line.len(),
                1_000,
                &StaticFormalsPolicy::default(),
            )
            .expect("R-only completion should succeed");
            assert_eq!(
                r_evaluation_count().saturating_sub(off_before),
                1,
                "off mode must use the R completion oracle"
            );

            let package_excluded = StaticFormalsPolicy {
                mode: StaticFormalsMode::PreferStatic,
                excluded_packages: vec!["stats".to_owned()],
                ..StaticFormalsPolicy::default()
            };
            let excluded_before = r_evaluation_count();
            let _ = get_completions_with_policy(line, line.len(), 1_000, &package_excluded)
                .expect("package exclusion should retain R completion");
            assert_eq!(
                r_evaluation_count().saturating_sub(excluded_before),
                1,
                "package exclusion must use the R completion oracle"
            );

            let function_excluded = StaticFormalsPolicy {
                mode: StaticFormalsMode::PreferStatic,
                excluded_functions: vec!["stats::lm".to_owned()],
                ..StaticFormalsPolicy::default()
            };
            let excluded_before = r_evaluation_count();
            let _ = get_completions_with_policy(line, line.len(), 1_000, &function_excluded)
                .expect("function exclusion should retain R completion");
            assert_eq!(
                r_evaluation_count().saturating_sub(excluded_before),
                1,
                "function exclusion must use the R completion oracle"
            );

            let result = lookup(line, line.len()).expect("qualified call should parse");
            let expected =
                production_candidates(&result).expect("stats::lm should be a static hit");
            let before = r_evaluation_count();
            let _ = get_token(line, line.len()).expect("token lookup should succeed");
            let after_token = r_evaluation_count();
            let actual = get_completions_with_policy(line, line.len(), 1_000, &policy)
                .expect("completion should succeed");
            let after_completion = r_evaluation_count();
            let _ = check_if_functions(&["lm"]).expect("function check should succeed");
            let after_functions = r_evaluation_count();
            assert_eq!(
                actual, expected,
                "static hit must return only formatted formals"
            );
            assert_eq!(
                [
                    after_token.saturating_sub(before),
                    after_completion.saturating_sub(after_token),
                    after_functions.saturating_sub(after_completion),
                    after_functions.saturating_sub(before),
                ],
                [1, 0, 1, 2],
                "static hit wrapper sequence must be token=1, completion=0, function-check=1"
            );

            for unsupported in [
                "stats:::lm(",
                "missing_package::foo(",
                "stats::lm(zz",
                "stats::lm(x = value[1, fo",
                "stats::lm(x = value[1, ",
                "stats::lm(x = { value[1, fo",
                "stats::lm(x = value[1], fo",
                "stats::lm(x = {1; 2}, fo",
            ] {
                let expected = get_r_completions(unsupported, unsupported.len(), 1_000)
                    .expect("R oracle should remain available");
                let before = r_evaluation_count();
                let _ =
                    get_token(unsupported, unsupported.len()).expect("token lookup should succeed");
                let after_token = r_evaluation_count();
                let actual =
                    get_completions_with_policy(unsupported, unsupported.len(), 1_000, &policy)
                        .expect("R fallback should remain available");
                let after_completion = r_evaluation_count();
                let _ = check_if_functions(&["foo"]).expect("function check should succeed");
                let after_functions = r_evaluation_count();
                assert_eq!(
                    [
                        after_token.saturating_sub(before),
                        after_completion.saturating_sub(after_token),
                        after_functions.saturating_sub(after_completion),
                        after_functions.saturating_sub(before),
                    ],
                    [1, 1, 1, 3],
                    "fallback wrapper sequence must be token=1, completion=1, function-check=1: {unsupported}"
                );
                assert_eq!(actual, expected, "R fallback candidates: {unsupported}");
            }
        },
    );
}
