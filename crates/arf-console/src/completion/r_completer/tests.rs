use super::*;

// --- Namespace token parsing tests ---

#[test]
fn test_parse_namespace_token_basic() {
    let result = parse_namespace_token("sf::geo", 7);
    assert_eq!(
        result,
        Some(NamespaceToken {
            package: "sf".to_string(),
            partial: "geo".to_string(),
            triple_colon: false,
            start_pos: 0,
        })
    );
}

#[test]
fn test_parse_namespace_token_triple_colon() {
    let result = parse_namespace_token("pkg:::func", 10);
    assert_eq!(
        result,
        Some(NamespaceToken {
            package: "pkg".to_string(),
            partial: "func".to_string(),
            triple_colon: true,
            start_pos: 0,
        })
    );
}

#[test]
fn test_parse_namespace_token_empty_partial() {
    let result = parse_namespace_token("stats::", 7);
    assert_eq!(
        result,
        Some(NamespaceToken {
            package: "stats".to_string(),
            partial: "".to_string(),
            triple_colon: false,
            start_pos: 0,
        })
    );
}

#[test]
fn test_parse_namespace_token_in_expression() {
    let result = parse_namespace_token("x <- dplyr::filt", 16);
    assert_eq!(
        result,
        Some(NamespaceToken {
            package: "dplyr".to_string(),
            partial: "filt".to_string(),
            triple_colon: false,
            start_pos: 5,
        })
    );
}

#[test]
fn test_parse_namespace_token_no_match() {
    // No :: at all
    assert_eq!(parse_namespace_token("hello", 5), None);
    // Just colons at end (no package)
    assert_eq!(parse_namespace_token("::", 2), None);
    // Empty line
    assert_eq!(parse_namespace_token("", 0), None);
}

#[test]
fn test_parse_namespace_token_dotted_package() {
    let result = parse_namespace_token("data.table::set", 15);
    assert_eq!(
        result,
        Some(NamespaceToken {
            package: "data.table".to_string(),
            partial: "set".to_string(),
            triple_colon: false,
            start_pos: 0,
        })
    );
}

// --- Backtick quoting tests ---

#[test]
fn test_needs_backtick_quoting() {
    // Syntactic names: no quoting needed
    assert!(!needs_backtick_quoting("filter"));
    assert!(!needs_backtick_quoting("st_geometry"));
    assert!(!needs_backtick_quoting("data.frame"));
    assert!(!needs_backtick_quoting(".internal"));
    assert!(!needs_backtick_quoting("my_func"));

    // Non-syntactic names: quoting needed
    assert!(needs_backtick_quoting("%>%"));
    assert!(needs_backtick_quoting("%in%"));
    assert!(needs_backtick_quoting("+.gg"));
    assert!(needs_backtick_quoting("[.data.frame"));
    assert!(needs_backtick_quoting("_private"));
    assert!(needs_backtick_quoting(".2bad"));

    // Edge cases: single dot and dotdot are syntactic
    assert!(!needs_backtick_quoting("."));
    assert!(!needs_backtick_quoting(".."));

    // Unicode: R syntactic names are ASCII-only, so Unicode requires quoting
    assert!(needs_backtick_quoting("données"));
    assert!(needs_backtick_quoting("日本語"));
    assert!(needs_backtick_quoting("café"));

    // Names with backticks: quoting needed (but unrepresentable in R syntax)
    assert!(needs_backtick_quoting("a`b"));
    assert!(needs_backtick_quoting("`"));

    // Empty
    assert!(!needs_backtick_quoting(""));
}

// --- Namespace cache tests ---

#[test]
fn test_namespace_cache_key_format() {
    assert_eq!(RCompleter::namespace_cache_key("dplyr", false), "dplyr::");
    assert_eq!(RCompleter::namespace_cache_key("dplyr", true), "dplyr:::");
}

#[test]
fn test_store_namespace_exports_caches_non_empty() {
    let mut completer = RCompleter::new();
    let exports = vec!["filter".to_string(), "mutate".to_string()];
    completer.store_namespace_exports("dplyr", false, exports.clone());

    let cached = completer.namespace_cache.get("dplyr::").unwrap();
    assert_eq!(cached.exports, exports);
}

#[test]
fn test_store_namespace_exports_skips_empty() {
    let mut completer = RCompleter::new();
    completer.store_namespace_exports("nonexistent", false, vec![]);

    assert!(!completer.namespace_cache.contains_key("nonexistent::"));
}

#[test]
fn test_store_namespace_exports_removes_stale_on_empty() {
    let mut completer = RCompleter::new();

    // First store a valid entry
    completer.store_namespace_exports("pkg", false, vec!["func".to_string()]);
    assert!(completer.namespace_cache.contains_key("pkg::"));

    // Storing empty should remove the existing entry
    completer.store_namespace_exports("pkg", false, vec![]);
    assert!(!completer.namespace_cache.contains_key("pkg::"));
}

#[test]
fn test_store_namespace_exports_evicts_expired() {
    let mut completer = RCompleter::new();

    // Insert an already-expired entry.
    // Use checked_sub to avoid overflow on Windows where Instant is relative to boot time
    // (system uptime may be shorter than cache duration in CI).
    let expired_duration = RCompleter::NAMESPACE_CACHE_DURATION + Duration::from_secs(1);
    let Some(expired_timestamp) = Instant::now().checked_sub(expired_duration) else {
        // System uptime too short to create an expired entry; skip test
        return;
    };
    completer.namespace_cache.insert(
        "old_pkg::".to_string(),
        NamespaceExportCache {
            exports: vec!["old_func".to_string()],
            timestamp: expired_timestamp,
        },
    );

    // Store a new entry — should evict the expired one
    completer.store_namespace_exports("new_pkg", false, vec!["new_func".to_string()]);

    assert!(!completer.namespace_cache.contains_key("old_pkg::"));
    assert!(completer.namespace_cache.contains_key("new_pkg::"));
}

#[test]
fn test_store_namespace_exports_keeps_fresh_entries() {
    let mut completer = RCompleter::new();

    // Insert a fresh entry for another package
    completer.store_namespace_exports("pkg_a", false, vec!["func_a".to_string()]);

    // Store a second package
    completer.store_namespace_exports("pkg_b", false, vec!["func_b".to_string()]);

    // Both should still be present
    assert!(completer.namespace_cache.contains_key("pkg_a::"));
    assert!(completer.namespace_cache.contains_key("pkg_b::"));
}

#[test]
fn test_separate_cache_for_double_and_triple_colon() {
    let mut completer = RCompleter::new();
    completer.store_namespace_exports("pkg", false, vec!["exported".to_string()]);
    completer.store_namespace_exports(
        "pkg",
        true,
        vec!["exported".to_string(), "internal".to_string()],
    );

    let double = completer.namespace_cache.get("pkg::").unwrap();
    assert_eq!(double.exports, vec!["exported"]);

    let triple = completer.namespace_cache.get("pkg:::").unwrap();
    assert_eq!(triple.exports, vec!["exported", "internal"]);
}

#[test]
fn test_invalidate_cache_preserves_namespace_cache() {
    let mut completer = RCompleter::new();
    completer.store_namespace_exports("dplyr", false, vec!["filter".to_string()]);

    completer.invalidate_cache();

    // Namespace export cache uses TTL-based expiry, not cleared by invalidate_cache
    assert!(completer.namespace_cache.contains_key("dplyr::"));
}

#[test]
fn test_invalidate_cache_clears_fuzzy_namespace_cache() {
    let mut completer = RCompleter::new();
    completer.namespace_fuzzy_cache = Some(NamespaceFuzzyCache {
        input: "dplyr::filt".to_string(),
        start_pos: 0,
        suggestions: vec![],
        timestamp: Instant::now(),
    });

    completer.invalidate_cache();

    assert!(completer.namespace_fuzzy_cache.is_none());
}

#[test]
fn test_fuzzy_cache_hit_same_input_and_position() {
    let mut completer = RCompleter::new();
    completer.debounce_ms = 5000;

    completer.namespace_fuzzy_cache = Some(NamespaceFuzzyCache {
        input: "dplyr::filt".to_string(),
        start_pos: 0,
        suggestions: vec![],
        timestamp: Instant::now(),
    });

    assert!(completer.is_namespace_fuzzy_cache_hit("dplyr::filt", 0));
}

#[test]
fn test_fuzzy_cache_miss_different_start_pos() {
    let mut completer = RCompleter::new();
    completer.debounce_ms = 5000;

    completer.namespace_fuzzy_cache = Some(NamespaceFuzzyCache {
        input: "dplyr::filt".to_string(),
        start_pos: 0,
        suggestions: vec![],
        timestamp: Instant::now(),
    });

    // Same input text but at a different position: must miss
    assert!(!completer.is_namespace_fuzzy_cache_hit("dplyr::filt", 5));
}

#[test]
fn test_fuzzy_cache_miss_different_input() {
    let mut completer = RCompleter::new();
    completer.debounce_ms = 5000;

    completer.namespace_fuzzy_cache = Some(NamespaceFuzzyCache {
        input: "dplyr::filt".to_string(),
        start_pos: 0,
        suggestions: vec![],
        timestamp: Instant::now(),
    });

    assert!(!completer.is_namespace_fuzzy_cache_hit("dplyr::filte", 0));
}

#[test]
fn test_fuzzy_cache_miss_when_empty() {
    let completer = RCompleter::new();
    assert!(!completer.is_namespace_fuzzy_cache_hit("dplyr::filt", 0));
}

#[test]
fn test_fuzzy_cache_miss_when_expired() {
    let mut completer = RCompleter::new();
    completer.debounce_ms = 0; // zero window: always expired

    completer.namespace_fuzzy_cache = Some(NamespaceFuzzyCache {
        input: "dplyr::filt".to_string(),
        start_pos: 0,
        suggestions: vec![],
        timestamp: Instant::now(),
    });

    assert!(!completer.is_namespace_fuzzy_cache_hit("dplyr::filt", 0));
}

// --- Library context detection tests ---

fn lib_funcs() -> Vec<String> {
    vec!["library".to_string(), "require".to_string()]
}

#[test]
fn test_detect_library_context_library() {
    let result = detect_library_context("library(dpl", 11, &lib_funcs());
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "dpl".to_string(),
            start_pos: 8,
        })
    );
}

#[test]
fn test_detect_library_context_require() {
    let result = detect_library_context("require(gg", 10, &lib_funcs());
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "gg".to_string(),
            start_pos: 8,
        })
    );
}

#[test]
fn test_detect_library_context_comma_skipped() {
    // Comma means we're past the first argument
    let result = detect_library_context("library(dplyr, ", 15, &lib_funcs());
    assert_eq!(result, None);
}

#[test]
fn test_detect_library_context_quoted_skipped() {
    let result = detect_library_context("library(\"dpl", 12, &lib_funcs());
    assert_eq!(result, None);
}

#[test]
fn test_detect_library_context_box_use() {
    let funcs = vec!["box::use".to_string()];
    let result = detect_library_context("box::use(dpl", 12, &funcs);
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "dpl".to_string(),
            start_pos: 9,
        })
    );
}

#[test]
fn test_detect_library_context_wrong_function() {
    let result = detect_library_context("foo(bar", 7, &lib_funcs());
    assert_eq!(result, None);
}

#[test]
fn test_detect_library_context_with_spaces() {
    let result = detect_library_context("  library( dpl", 14, &lib_funcs());
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "dpl".to_string(),
            start_pos: 11,
        })
    );
}

#[test]
fn test_detect_library_context_empty_partial() {
    let result = detect_library_context("x <- library(", 13, &lib_funcs());
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "".to_string(),
            start_pos: 13,
        })
    );
}

#[test]
fn test_detect_library_context_nested_parens() {
    // `print(library(dpl` — cursor is inside library()
    let result = detect_library_context("print(library(dpl", 17, &lib_funcs());
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "dpl".to_string(),
            start_pos: 14,
        })
    );
}

#[test]
fn test_detect_library_context_single_quote_skipped() {
    let result = detect_library_context("library('dpl", 12, &lib_funcs());
    assert_eq!(result, None);
}

#[test]
fn test_detect_library_context_stray_colons_no_match() {
    // Stray colons before function name — no match (benign)
    assert_eq!(
        detect_library_context("x:library(dpl", 13, &lib_funcs()),
        None
    );
    assert_eq!(
        detect_library_context(":::library(dpl", 14, &lib_funcs()),
        None
    );
}

#[test]
fn test_detect_library_context_non_ascii() {
    // 'é' is alphanumeric per Rust's char::is_alphanumeric, so it's included in the partial
    let result = detect_library_context("library(données", 16, &lib_funcs());
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "données".to_string(),
            start_pos: 8,
        })
    );
}

#[test]
fn test_detect_library_context_utf8_boundary_safety() {
    // cursor_pos in the middle of a multi-byte char should return None, not panic
    let line = "library(données";
    // 'é' is 2 bytes in UTF-8, find a mid-byte position
    let e_pos = line.find('é').unwrap();
    let mid_byte = e_pos + 1; // middle of 'é'
    assert!(!line.is_char_boundary(mid_byte));
    assert_eq!(detect_library_context(line, mid_byte, &lib_funcs()), None);
}

#[test]
fn test_detect_library_context_member_access_skipped() {
    // obj$library( and env@require( are member accesses, not function calls
    assert_eq!(
        detect_library_context("obj$library(dpl", 15, &lib_funcs()),
        None
    );
    assert_eq!(
        detect_library_context("env@require(gg", 14, &lib_funcs()),
        None
    );
}

#[test]
fn test_detect_library_context_namespace_in_arg() {
    // library(pkg::something) — partial stops at `:`, span covers full range.
    // This is invalid R, but documenting the behavior: partial is "pkg",
    // span is start_pos..cursor_pos (covering "pkg::something").
    let result = detect_library_context("library(pkg::something", 22, &lib_funcs());
    assert_eq!(
        result,
        Some(LibraryContext {
            partial: "pkg".to_string(),
            start_pos: 8,
        })
    );
}

// --- Prefix extension cache tests ---

fn completer_with_cache(token: &str, completions: Vec<String>) -> RCompleter {
    let mut c = RCompleter::new();
    c.cache = Some(CompletionCache {
        token: token.to_string(),
        completions,
        timestamp: Instant::now(),
    });
    c
}

#[test]
fn test_is_prefix_extension_identifier_only() {
    // "pri" extends "pr" — pure identifier extension, cache is valid
    let c = completer_with_cache("pr", vec!["print".to_string()]);
    assert!(c.is_prefix_extension("pri"));
}

#[test]
fn test_is_prefix_extension_dot_underscore() {
    // "my_f" extends "my_" and "st.g" extends "st." — dots/underscores are ok
    let c = completer_with_cache("my_", vec!["my_func".to_string()]);
    assert!(c.is_prefix_extension("my_f"));

    let c = completer_with_cache("st.", vec!["st.geo".to_string()]);
    assert!(c.is_prefix_extension("st.g"));
}

#[test]
fn test_is_prefix_extension_dollar_sign_not_extension() {
    // "l$a$" extends "l$" by "a$" — the `$` means a new $ access context;
    // cached completions for "l$" ([l$a, l$b]) are irrelevant for "l$a$".
    // This is the nested-list completion bug: must return false so fresh
    // completions are fetched.
    let c = completer_with_cache("l$", vec!["l$a".to_string(), "l$b".to_string()]);
    assert!(!c.is_prefix_extension("l$a$"));
}

#[test]
fn test_is_prefix_extension_at_sign_not_extension() {
    // "obj@field$" extends "obj@" by "field$" — structural operator
    let c = completer_with_cache("obj@", vec!["obj@slot".to_string()]);
    assert!(!c.is_prefix_extension("obj@slot$"));
}

#[test]
fn test_is_prefix_extension_same_token_not_extension() {
    let c = completer_with_cache("pri", vec!["print".to_string()]);
    assert!(!c.is_prefix_extension("pri"));
}

#[test]
fn test_is_prefix_extension_bracket_not_extension() {
    // "l[" extends "l" by "[" — bracket access changes completion context
    let c = completer_with_cache("l", vec!["list".to_string()]);
    assert!(!c.is_prefix_extension("l["));
}

#[test]
fn test_is_prefix_extension_colon_not_extension() {
    // "pkg::foo" extends "pkg:" by ":foo" — namespace operator, fresh fetch needed
    let c = completer_with_cache("pkg:", vec!["pkg::foo".to_string()]);
    assert!(!c.is_prefix_extension("pkg::foo"));
}

#[test]
fn test_is_prefix_extension_no_cache() {
    let c = RCompleter::new();
    assert!(!c.is_prefix_extension("pri"));
}
