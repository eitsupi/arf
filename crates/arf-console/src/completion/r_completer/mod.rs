//! R code completer using R's built-in completion functions with fuzzy namespace support.

mod context;
mod namespace;
mod quoting;

use super::string_context::{complete_path_in_string, detect_string_context};
use context::{CompletionContext, completion_context, detect_library_context};
use namespace::{NamespaceExportCache, NamespaceFuzzyCache, parse_namespace_token};
use quoting::has_special_suffix;
use reedline::{Completer, CompletionResult, Span, Suggestion};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Cache for completion results to avoid repeated R calls.
#[derive(Debug, Clone)]
struct CompletionCache {
    /// The token that was used for the R completion query.
    token: String,
    /// The input before the token, used to identify the completion context.
    prefix: String,
    /// The input after the cursor, used to identify the completion context.
    suffix: String,
    /// The full completion results from R.
    completions: Vec<String>,
    /// When the completion was fetched.
    timestamp: Instant,
}

/// R code completer using R's built-in completion functions.
pub struct RCompleter {
    /// Timeout in milliseconds for R completion (0 = no timeout).
    timeout_ms: u64,
    /// Debounce delay in milliseconds.
    debounce_ms: u64,
    /// Number of completions to check for function type (0 = disable).
    auto_paren_limit: usize,
    /// Cached completion results.
    cache: Option<CompletionCache>,
    /// Whether fuzzy namespace completion is enabled.
    fuzzy_namespace: bool,
    /// Function names that trigger package-name completion (e.g., "library", "require").
    package_functions: Vec<String>,
    /// Runtime policy for installed-package static formal completion.
    static_formals: arf_harp::completion::StaticFormalsPolicy,
    /// Per-package cache of namespace exports.
    namespace_cache: HashMap<String, NamespaceExportCache>,
    /// Debounce cache for fuzzy namespace completion results.
    namespace_fuzzy_cache: Option<NamespaceFuzzyCache>,
}

impl RCompleter {
    /// Create a new RCompleter with default settings (50ms timeout, 100ms debounce, 50 function check limit).
    pub fn new() -> Self {
        RCompleter {
            timeout_ms: 50,
            debounce_ms: 100,
            auto_paren_limit: 50,
            cache: None,
            fuzzy_namespace: false,
            package_functions: vec!["library".to_string(), "require".to_string()],
            static_formals: arf_harp::completion::StaticFormalsPolicy::off(),
            namespace_cache: HashMap::new(),
            namespace_fuzzy_cache: None,
        }
    }

    /// Create a completer with all settings, including static formal policy.
    pub fn with_settings_full_and_static_formals(
        timeout_ms: u64,
        debounce_ms: u64,
        auto_paren_limit: usize,
        fuzzy_namespace: bool,
        package_functions: Vec<String>,
        static_formals: arf_harp::completion::StaticFormalsPolicy,
    ) -> Self {
        RCompleter {
            timeout_ms,
            debounce_ms,
            auto_paren_limit,
            cache: None,
            fuzzy_namespace,
            package_functions,
            static_formals,
            namespace_cache: HashMap::new(),
            namespace_fuzzy_cache: None,
        }
    }

    /// Check if the new token extends the cached token (prefix extension).
    fn is_prefix_extension(&self, context: &CompletionContext) -> bool {
        if let Some(cache) = &self.cache {
            if cache.prefix != context.prefix || cache.suffix != context.suffix {
                return false;
            }
            // Only reuse the cache when the extension consists solely of identifier
            // characters (alphanumeric, `.`, `_`).  Structural operators such as `$`,
            // `@`, `[`, or `:` change the completion context entirely, so a fresh
            // fetch is required.
            // Example: "l$a$" extends "l$" by "a$" — the `$` means we are now
            // completing inside a nested list and the old completions are irrelevant.
            let Some(extension) = context.token.strip_prefix(cache.token.as_str()) else {
                return false;
            };
            !extension.is_empty()
                && extension
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
        } else {
            false
        }
    }

    /// Check if we should use cached results (within debounce window, same token).
    fn should_use_cache(&self, context: &CompletionContext) -> bool {
        if let Some(cache) = &self.cache {
            // Don't use empty cache
            if cache.completions.is_empty() {
                return false;
            }

            // Don't use cache for empty tokens (context likely changed)
            if context.token.is_empty() || cache.token.is_empty() {
                return false;
            }

            // A token alone is not a sufficient cache key: qualified calls,
            // named arguments, and cursor suffixes can all change its meaning.
            if cache.prefix != context.prefix || cache.suffix != context.suffix {
                return false;
            }

            // Don't use cache for package:: completions (always fetch fresh, worth waiting)
            if context.token.contains("::") || cache.token.contains("::") {
                return false;
            }

            // Use cache if:
            // 1. Same token and within debounce window
            // 2. Token is a prefix extension of cached token
            if cache.token == context.token {
                cache.timestamp.elapsed() < Duration::from_millis(self.debounce_ms)
            } else {
                self.is_prefix_extension(context)
            }
        } else {
            false
        }
    }

    /// Filter cached completions for a new token (prefix extension).
    fn filter_cached(&self, token: &str) -> Vec<String> {
        if let Some(cache) = &self.cache {
            cache
                .completions
                .iter()
                .filter(|c| c.starts_with(token))
                .cloned()
                .collect()
        } else {
            vec![]
        }
    }

    /// Invalidate the completion cache.
    ///
    /// Only clears the debounce/prefix cache. Namespace export cache is
    /// preserved since package exports are stable and use TTL-based expiry.
    fn invalidate_cache(&mut self) {
        self.cache = None;
        self.namespace_fuzzy_cache = None;
    }
}

impl Default for RCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for RCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        CompletionResult::fresh(self.complete_impl(line, pos))
    }
}

impl RCompleter {
    fn complete_impl(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        // Check if we're inside a string - use Rust path completion with fuzzy matching
        // This provides better UX than R's built-in completion:
        // - Fuzzy matching: "rdm" matches "README.md"
        // - Faster: no R evaluation needed
        // - Hidden files shown last
        if let Some(ctx) = detect_string_context(line, pos) {
            return complete_path_in_string(line, pos, &ctx);
        }

        // Fuzzy namespace completion: intercept pkg::partial before R's built-in
        if self.fuzzy_namespace
            && let Some(ns_token) = parse_namespace_token(line, pos)
        {
            let input = &line[ns_token.start_pos..pos];

            // Debounce: reuse cached results if same input at same position within window
            if self.is_namespace_fuzzy_cache_hit(input, ns_token.start_pos) {
                return self
                    .namespace_fuzzy_cache
                    .as_ref()
                    .unwrap()
                    .suggestions
                    .clone();
            }

            let suggestions = self.complete_namespace_fuzzy(&ns_token, pos);

            // Cache the results
            self.namespace_fuzzy_cache = Some(NamespaceFuzzyCache {
                input: input.to_string(),
                start_pos: ns_token.start_pos,
                suggestions: suggestions.clone(),
                timestamp: Instant::now(),
            });

            return suggestions;
        }

        // Fuzzy library completion: intercept library()/require() before R's built-in
        if self.fuzzy_namespace
            && let Some(lib_ctx) = detect_library_context(line, pos, &self.package_functions)
        {
            return self.complete_library_fuzzy(&lib_ctx, pos);
        }

        // Get the token being completed (for filtering and span calculation)
        let token = arf_harp::completion::get_token(line, pos).unwrap_or_default();
        let context = completion_context(line, pos, &token);

        // Try to use cache for prefix extensions or debounced requests
        let completions = if context
            .as_ref()
            .is_some_and(|context| self.should_use_cache(context))
        {
            // Use cached results, filtering for the current token
            self.filter_cached(&token)
        } else {
            // Fetch fresh completions using the configured static/R policy.
            let fresh = match arf_harp::completion::get_completions_with_policy(
                line,
                pos,
                self.timeout_ms,
                &self.static_formals,
            ) {
                Ok(c) => c,
                Err(_) => {
                    // On error, invalidate cache and return empty
                    self.invalidate_cache();
                    return vec![];
                }
            };

            // Only cache non-empty results with non-empty tokens
            if !fresh.is_empty()
                && !token.is_empty()
                && let Some(context) = context
            {
                self.cache = Some(CompletionCache {
                    token: context.token,
                    prefix: context.prefix,
                    suffix: context.suffix,
                    completions: fresh.clone(),
                    timestamp: Instant::now(),
                });
            }

            fresh
        };

        if completions.is_empty() {
            return vec![];
        }

        // Filter completions: include if starts with token and (different from token OR is a function)
        // This allows "foo" -> "foo(" completion when foo is a function
        let filtered: Vec<String> = completions
            .into_iter()
            .filter(|c| c.starts_with(&token))
            .collect();

        if filtered.is_empty() {
            return vec![];
        }

        // Determine which completions are functions (for parenthesis insertion)
        let is_function = if self.auto_paren_limit > 0 {
            self.check_function_types(&filtered)
        } else {
            vec![false; filtered.len()]
        };

        // Convert to reedline Suggestions
        let match_len = token.len();
        filtered
            .into_iter()
            .zip(is_function)
            .filter(|(c, is_func)| {
                // Include if: different from token, OR is a function (will get "(" added)
                c != &token || (*is_func && !has_special_suffix(c))
            })
            .map(|(c, is_func)| {
                let start = pos - token.len();
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };

                // Add "()" suffix for functions
                // FunctionAwareMenu will move cursor back to place it inside: foo(|)
                let (value, extra_info) = if is_func && !has_special_suffix(&c) {
                    (format!("{}()", c), Some("function".to_string()))
                } else {
                    (c, None)
                };

                Suggestion {
                    value,
                    display_override: None,
                    description: extra_info,
                    extra: None,
                    span: Span { start, end: pos },
                    append_whitespace: false,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect()
    }
}

impl RCompleter {
    /// Check which completions are functions.
    ///
    /// Only checks up to `auto_paren_limit` items for performance.
    fn check_function_types(&self, completions: &[String]) -> Vec<bool> {
        // Only check the first N items
        let check_count = completions.len().min(self.auto_paren_limit);

        if check_count == 0 {
            return vec![false; completions.len()];
        }

        // Collect names to check (exclude items with special suffixes)
        let names_to_check: Vec<&str> = completions[..check_count]
            .iter()
            .map(|s| {
                if has_special_suffix(s) {
                    // Don't check items with special suffixes
                    ""
                } else {
                    s.as_str()
                }
            })
            .collect();

        // Call R to check function types
        let checked = arf_harp::completion::check_if_functions(&names_to_check).unwrap_or_default();

        // Extend with false for remaining items (not checked)
        let mut result = checked;
        result.resize(completions.len(), false);
        result
    }
}

#[cfg(test)]
mod tests;
