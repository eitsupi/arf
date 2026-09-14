//! Re-runnable observations for the `rd-rds` installed-package inspection API.
//!
//! This is intentionally an ignored integration test: it is an experiment
//! against whichever R library is available on the host, not a deterministic
//! completion regression test.

#![cfg(feature = "completion-spike")]

mod common;

#[cfg(feature = "experimental-static-formals")]
use arf_harp::completion::static_formals::production_candidates;
use arf_harp::completion::static_formals::{
    StaticFormalsOutcome, candidates, lookup, r_evaluation_count, shadow,
};
use arf_harp::completion::{check_if_functions, get_completions, get_token};
use arf_harp::{eval_string_with_visibility, lib_paths};
use common::{ld_library_path_is_set, with_r};
use std::time::Instant;

#[test]
#[ignore = "requires an installed R package corpus and is an observation harness"]
fn static_formals_integration_spike() {
    if !ld_library_path_is_set() {
        eprintln!("Skipping static formals spike: LD_LIBRARY_PATH is not set.");
        return;
    }

    with_r(|| {
        lib_paths::populate_lib_paths().expect("R library paths should be available");
        let installed = arf_harp::completion::get_installed_packages()
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
            let cold_started = Instant::now();
            let cold = lookup(&line, cursor).expect("qualified call should parse");
            let cold_ms = cold_started.elapsed().as_secs_f64() * 1_000.0;

            let mut warm_samples = Vec::new();
            for _ in 0..5 {
                let started = Instant::now();
                let _ = lookup(&line, cursor);
                warm_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
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
            let oracle = shadow(line, cursor, 1_000);
            let after_oracle = namespace_state(package);
            let (r_before_candidates, r_before_ms, r_calls, r_eval_count) = match oracle {
                Ok(Some(observation)) => (
                    observation.r_candidates,
                    observation.r_elapsed.as_secs_f64() * 1_000.0,
                    observation.r_oracle_calls,
                    observation.r_evaluation_count,
                ),
                Ok(None) => (Vec::new(), 0.0, 0, None),
                Err(error) => {
                    eprintln!("{package}::{function} ({line}): R oracle failed: {error}");
                    (Vec::new(), 0.0, 0, None)
                }
            };
            assert_eq!(
                r_eval_count,
                Some(1),
                "shadow must invoke exactly one R oracle callback"
            );
            let _ = eval_string_with_visibility(&format!(
                "try(loadNamespace({package:?}), silent = TRUE)"
            ));
            let after_load = namespace_state(package);
            let after_load_oracle = shadow(line, cursor, 1_000);
            let after_after_load_oracle = namespace_state(package);
            let (r_after_candidates, r_after_ms, r_after_eval_count) = match after_load_oracle {
                Ok(Some(observation)) => (
                    observation.r_candidates,
                    observation.r_elapsed.as_secs_f64() * 1_000.0,
                    observation.r_evaluation_count,
                ),
                Ok(None) => (Vec::new(), 0.0, None),
                Err(error) => {
                    eprintln!(
                        "{package}::{function} ({line}): after-load R oracle failed: {error}"
                    );
                    (Vec::new(), 0.0, None)
                }
            };
            assert_eq!(
                r_after_eval_count,
                Some(1),
                "post-load shadow must invoke exactly one R oracle callback"
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
            // These are exact embedded-R callback counts when the feature is
            // enabled; normal builds contain no counter.
            let console_before = r_evaluation_count();
            let _ = get_token(&line, cursor);
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
            let eval_count = format!(
                "before {}, after {}",
                r_eval_count.map_or_else(
                    || format!("unavailable ({r_calls} oracle call)"),
                    |count| count.to_string()
                ),
                r_after_eval_count
                    .map_or_else(|| "unavailable".to_owned(), |count| count.to_string())
            );
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
    });
}

#[test]
#[ignore = "requires an installed R package corpus and both completion spike features"]
#[cfg(feature = "experimental-static-formals")]
fn experimental_static_first_hit_and_fallback() {
    if !ld_library_path_is_set() {
        eprintln!("Skipping experimental static-first test: LD_LIBRARY_PATH is not set.");
        return;
    }

    with_r(|| {
        lib_paths::populate_lib_paths().expect("R library paths should be available");
        let line = "stats::lm(";
        let result = lookup(line, line.len()).expect("qualified call should parse");
        let expected = production_candidates(&result).expect("stats::lm should be a static hit");
        let before = r_evaluation_count();
        let _ = get_token(line, line.len()).expect("token lookup should succeed");
        let after_token = r_evaluation_count();
        let actual = get_completions(line, line.len(), 1_000).expect("completion should succeed");
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

        for unsupported in ["stats:::lm(", "missing_package::foo("] {
            let before = r_evaluation_count();
            let _ = get_token(unsupported, unsupported.len()).expect("token lookup should succeed");
            let after_token = r_evaluation_count();
            let _ = get_completions(unsupported, unsupported.len(), 1_000)
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
                "fallback wrapper sequence must be token=1, completion=1, function-check=1"
            );
        }
    });
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
    unsafe { arf_harp::help::eval_r_to_string(&code) }
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".to_owned())
}
