//! R-dependent completion measurements, isolated from other unit tests.
//!
//! Run with an installed R package corpus (on Linux, set LD_LIBRARY_PATH to
//! include R's library directory):
//! `cargo test -p arf-harp --lib completion::r_tests -- --ignored --nocapture`
//! No Cargo feature is required. Each test runs in a fresh child process.

use super::r_ffi::r_evaluation_count;
use super::static_formals::{StaticFormalsOutcome, candidates, lookup, production_candidates};
use super::{
    StaticFormalsMode, StaticFormalsPolicy, check_if_functions, get_completions,
    get_completions_with_policy, get_token,
};
use crate::test_support::with_r_in_subprocess;
use crate::{eval_string_with_visibility, lib_paths};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires an installed R package corpus and is an observation harness"]
fn static_formals_integration_spike() {
    with_r_in_subprocess(
        concat!(module_path!(), "::static_formals_integration_spike"),
        || {
            lib_paths::populate_lib_paths().expect("R library paths should be available");
            let installed = crate::completion::get_installed_packages()
                .expect("installed package discovery should not error");
            let mut cases = vec![
                ("stats", "lm", "stats::lm("),
                ("stats", "lm", "stats::lm(fo"),
                ("stats", "glm", "stats::glm("),
                ("Matrix", "Matrix", "Matrix::Matrix("),
                ("dplyr", "mutate", "dplyr::mutate("),
                ("dplyr", "mutate", "dplyr::mutate(.d"),
                ("rlang", "abort", "rlang::abort("),
                ("rlang", "abort", "rlang::abort(c"),
                ("R6", "R6Class", "R6::R6Class("),
            ];
            cases.retain(|(package, _, _)| installed.iter().any(|name| name == package));

            println!("r-documentation-rs 0.5.0-alpha.1 static formals spike");
            println!("packages in corpus: {}", installed.len());
            println!(
                "| package/function | input | initial namespace | export resolved | stored kind | static formals/candidates | fallback reason | static cold ms | static warm p50 ms | R before ms/candidates | R after-load ms/candidates | agreement before/after | R eval before/after |"
            );
            println!("|---|---|---|---:|---|---|---|---:|---:|---:|---:|---|---|");

            for (package, function, line) in cases {
                let cursor = line.len();
                // Observe the actual initial state. Static inspection does not
                // load namespaces or evaluate R.
                let before = namespace_state(package);
                let static_count_before = r_evaluation_count();
                let cold_started = Instant::now();
                let cold = lookup(line, cursor).expect("qualified call should parse");
                let cold_ms = cold_started.elapsed().as_secs_f64() * 1_000.0;

                let mut warm_samples = Vec::new();
                for _ in 0..5 {
                    let started = Instant::now();
                    let _ = lookup(line, cursor);
                    warm_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
                }
                assert_eq!(
                    r_evaluation_count() - static_count_before,
                    0,
                    "static lookups must not invoke completion R callbacks"
                );
                warm_samples.sort_by(f64::total_cmp);
                let warm_p50 = warm_samples[warm_samples.len() / 2];

                let static_kind = cold
                    .kind
                    .map_or_else(|| "-".to_owned(), |kind| format!("{kind:?}"));
                let static_formals = match &cold.outcome {
                    StaticFormalsOutcome::Available(formals) => formals.len().to_string(),
                    _ => "-".to_owned(),
                };
                let static_candidates = candidates(&cold).unwrap_or_default();

                // Observe state after static inspection and after the R oracle.
                let after_static = namespace_state(package);
                let oracle = observe_r_completion(line, cursor);
                let after_oracle = namespace_state(package);
                let r_before_candidates = oracle.candidates;
                let r_before_ms = oracle.elapsed.as_secs_f64() * 1_000.0;
                let r_eval_count = oracle.evaluations;
                assert_eq!(
                    r_eval_count, 1,
                    "oracle must invoke one R evaluation callback"
                );
                let _ = eval_string_with_visibility(&format!(
                    "try(loadNamespace({package:?}), silent = TRUE)"
                ));
                let after_load = namespace_state(package);
                let after_load_oracle = observe_r_completion(line, cursor);
                let after_after_load_oracle = namespace_state(package);
                let r_after_candidates = after_load_oracle.candidates;
                let r_after_ms = after_load_oracle.elapsed.as_secs_f64() * 1_000.0;
                let r_after_eval_count = after_load_oracle.evaluations;
                assert_eq!(
                    r_after_eval_count, 1,
                    "post-load oracle must invoke one R evaluation callback"
                );
                let agreement = if static_candidates.is_empty() {
                    "n/a".to_owned()
                } else {
                    let exact_overlap = static_candidates
                        .iter()
                        .filter(|candidate| r_before_candidates.iter().any(|r| r == *candidate))
                        .count();
                    let before_name_overlap = static_candidates
                        .iter()
                        .filter(|candidate| {
                            r_before_candidates
                                .iter()
                                .any(|r| r_candidate_name(r) == candidate.as_str())
                        })
                        .count();
                    let after_name_overlap = static_candidates
                        .iter()
                        .filter(|candidate| {
                            r_after_candidates
                                .iter()
                                .any(|r| r_candidate_name(r) == candidate.as_str())
                        })
                        .count();
                    let after_exact_overlap = static_candidates
                        .iter()
                        .filter(|candidate| r_after_candidates.iter().any(|r| r == *candidate))
                        .count();
                    format!(
                        "before exact {exact_overlap}/{}, name-only {before_name_overlap}/{}; after exact {after_exact_overlap}/{}, name-only {after_name_overlap}/{}",
                        static_candidates.len(),
                        static_candidates.len(),
                        static_candidates.len(),
                        static_candidates.len()
                    )
                };

                // Measure the wrappers used by the console request separately.
                // Only callbacks in completion/r_ffi.rs are counted. Normal builds
                // contain no counter, and this is not the full console pipeline.
                let console_before = r_evaluation_count();
                let _ = get_token(line, cursor);
                let console_after_token = r_evaluation_count();
                let _ = get_completions(line, cursor, 1_000);
                let console_after_completion = r_evaluation_count();
                let _ = check_if_functions(&[function]);
                let console_after_functions = r_evaluation_count();
                let reason = match &cold.outcome {
                    StaticFormalsOutcome::Available(_) => "-".to_owned(),
                    StaticFormalsOutcome::NotApplicable(reason)
                    | StaticFormalsOutcome::Unavailable(reason)
                    | StaticFormalsOutcome::Unresolved(reason) => reason.clone(),
                };
                let eval_count = format!("before {r_eval_count}, after {r_after_eval_count}");
                println!(
                    "| {package}::{function} | `{line}` | {before} | {} | {static_kind} | {static_formals}/{} | {reason} | {cold_ms:.3} | {warm_p50:.3} | {r_before_ms:.3}/{} | {r_after_ms:.3}/{} | {agreement} | {eval_count} |",
                    cold.source_name.is_some(),
                    static_candidates.len(),
                    r_before_candidates.len(),
                    r_after_candidates.len(),
                );
                println!(
                    "  namespace state {package}: initial={before}, after_static={after_static}, after_before_load_oracle={after_oracle}, after_explicit_load={after_load}, after_after_load_oracle={after_after_load_oracle}"
                );
                println!(
                    "  console-like wrapper eval callbacks: get_token={}, get_completions={}, check_if_functions={} (total={})",
                    console_after_token.saturating_sub(console_before),
                    console_after_completion.saturating_sub(console_after_token),
                    console_after_functions.saturating_sub(console_after_completion),
                    console_after_functions.saturating_sub(console_before),
                );
            }
        },
    );
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
                "stats::lm(x = value[1, fo",
                "stats::lm(x = value[1, ",
                "stats::lm(x = { value[1, fo",
                "stats::lm(x = value[1], fo",
                "stats::lm(x = {1; 2}, fo",
            ] {
                let expected = get_completions(unsupported, unsupported.len(), 1_000)
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

struct RCompletionObservation {
    candidates: Vec<String>,
    elapsed: Duration,
    evaluations: usize,
}

fn observe_r_completion(line: &str, cursor: usize) -> RCompletionObservation {
    let count_before = r_evaluation_count();
    let started = Instant::now();
    // Always use the R oracle, independently of the static production policy.
    let candidates =
        super::get_r_completions(line, cursor, 1_000).expect("R completion oracle should succeed");
    RCompletionObservation {
        candidates,
        elapsed: started.elapsed(),
        evaluations: r_evaluation_count() - count_before,
    }
}

fn r_candidate_name(candidate: &str) -> &str {
    candidate
        .trim_end()
        .strip_suffix('=')
        .map(str::trim_end)
        .unwrap_or(candidate)
}

fn namespace_state(package: &str) -> String {
    let code = format!("if (isNamespaceLoaded({package:?})) 'loaded' else 'not-loaded'");
    // `eval_string_with_visibility` is the existing test-only R harness; this
    // observation does not affect the static provider or normal completion.
    unsafe { crate::help::eval_r_to_string(&code) }
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".to_owned())
}
