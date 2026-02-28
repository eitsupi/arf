//! R code completer using R's built-in completion functions with fuzzy namespace support.

use super::string_context::{complete_path_in_string, detect_string_context};
use crate::fuzzy::fuzzy_match;
use reedline::{Completer, Span, Suggestion};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Cache for completion results to avoid repeated R calls.
#[derive(Debug, Clone)]
struct CompletionCache {
    /// The token that was used for the R completion query.
    token: String,
    /// The full completion results from R.
    completions: Vec<String>,
    /// When the completion was fetched.
    timestamp: Instant,
}

/// Parsed `pkg::partial` or `pkg:::partial` token from user input.
#[derive(Debug, PartialEq)]
struct NamespaceToken {
    /// Package name (e.g., "sf").
    package: String,
    /// Partial function name after `::` (e.g., "geo"). May be empty.
    partial: String,
    /// Whether `:::` was used (internal access).
    triple_colon: bool,
    /// Byte position in the line where the token starts (beginning of `pkg`).
    start_pos: usize,
}

/// Parse a `pkg::partial` or `pkg:::partial` pattern from the text before the cursor.
///
/// Returns `None` if no namespace pattern is found.
fn parse_namespace_token(line: &str, cursor_pos: usize) -> Option<NamespaceToken> {
    let before_cursor = &line[..cursor_pos.min(line.len())];

    // Find `::` or `:::` by scanning backwards from cursor
    // First, extract the partial (identifier chars at the end)
    let partial: String = before_cursor
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();

    let before_partial = &before_cursor[..before_cursor.len() - partial.len()];

    // Check for `:::` or `::`
    let (triple_colon, before_colons) = if let Some(rest) = before_partial.strip_suffix(":::") {
        (true, rest)
    } else if let Some(rest) = before_partial.strip_suffix("::") {
        (false, rest)
    } else {
        return None;
    };

    // Extract package name (identifier chars before the colons)
    let package: String = before_colons
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();

    if package.is_empty() {
        return None;
    }

    // Validate: R identifiers can't start with a digit
    if package.chars().next()?.is_ascii_digit() {
        return None;
    }

    let colon_len = if triple_colon { 3 } else { 2 };
    let start_pos = before_colons.len() - package.len();

    // Sanity check: start_pos should be within bounds
    if start_pos + package.len() + colon_len + partial.len() > cursor_pos {
        return None;
    }

    Some(NamespaceToken {
        package,
        partial,
        triple_colon,
        start_pos,
    })
}

/// Cache entry for namespace exports.
struct NamespaceExportCache {
    exports: Vec<String>,
    timestamp: Instant,
}

/// Cache for fuzzy namespace completion results (debounce).
struct NamespaceFuzzyCache {
    /// The full input used for this cache (e.g., "pkg::partial").
    input: String,
    /// Start position of the namespace token in the line.
    start_pos: usize,
    /// Cached suggestions.
    suggestions: Vec<Suggestion>,
    /// When the cache was created.
    timestamp: Instant,
}

/// Check if an R name requires backtick quoting (non-syntactic name).
///
/// Names that start with an ASCII letter or `.` followed by a non-digit, and
/// contain only ASCII alphanumeric, `.`, or `_` characters are syntactic.
/// Everything else (operators like `%>%`, names starting with `_` or digits,
/// names with non-ASCII or special characters) requires backtick quoting.
///
/// This is intentionally conservative (ASCII-only) to match R's default
/// parser behavior where non-ASCII identifiers require backtick quoting.
fn needs_backtick_quoting(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let mut chars = name.chars();
    let first = chars.next().unwrap();

    // Must start with a letter or '.'
    if !first.is_ascii_alphabetic() && first != '.' {
        return true;
    }

    // If starts with '.', second char must not be a digit
    if first == '.'
        && let Some(second) = chars.next()
    {
        if second.is_ascii_digit() {
            return true;
        }
        // Check remaining chars after second
        if !second.is_ascii_alphanumeric() && second != '.' && second != '_' {
            return true;
        }
    }

    // All remaining characters must be alphanumeric, '.', or '_'
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '.' && c != '_' {
            return true;
        }
    }

    false
}

/// Check if a completion has a special suffix that means it shouldn't get parentheses.
fn has_special_suffix(s: &str) -> bool {
    // Package namespace (already has ::)
    s.ends_with("::")
        // Assignment operators
        || s.ends_with("<-")
        || s.ends_with("<<-")
        // Already has parentheses
        || s.ends_with("()")
        || s.ends_with('(')
        // File paths (R completion returns these for strings)
        || s.ends_with('/')
        // Argument names (completion inside function calls)
        || s.ends_with(" = ")
        || s.ends_with("=")
}

/// Context for a detected `library(partial)` or similar call.
#[derive(Debug, PartialEq)]
struct LibraryContext {
    /// The partial package name being typed (may be empty).
    partial: String,
    /// Byte position where the partial starts (after `(` + whitespace).
    start_pos: usize,
}

/// Detect if the cursor is inside a `library()`, `require()`, or user-configured function call.
///
/// Scans backwards from cursor to find the last unmatched `(`, extracts the function name,
/// and checks it against `func_names`. Returns `None` if:
/// - No unmatched `(` found
/// - Function name doesn't match any in `func_names`
/// - There's a comma after `(` (not first argument)
/// - The argument starts with a quote (string argument)
///
/// # Limitation
///
/// The backward parenthesis scan does not track string literal boundaries, so a `)` inside
/// a string literal is counted as a real closing paren. In theory this could miscount
/// paren depth, but in practice it is unreachable for `library()`/`require()` because:
/// - These functions take a bare symbol as the first argument, not nested expressions.
/// - Any nested call containing a string with `)` would also contain commas,
///   which triggers the early `None` return (first-argument-only guard).
/// - A `)` in a string *before* the target `(` is never reached, since the scan
///   stops at the nearest unmatched `(`.
///
/// The same approach is used in `arf-harp/src/completion.rs`.
fn detect_library_context(
    line: &str,
    cursor_pos: usize,
    func_names: &[String],
) -> Option<LibraryContext> {
    let pos = cursor_pos.min(line.len());
    if !line.is_char_boundary(pos) {
        return None;
    }
    let before_cursor = &line[..pos];

    // Find the last unmatched opening parenthesis before cursor
    let mut paren_depth = 0;
    let mut last_open_paren_pos = None;

    for (i, c) in before_cursor.char_indices().rev() {
        match c {
            ')' => paren_depth += 1,
            '(' => {
                if paren_depth == 0 {
                    last_open_paren_pos = Some(i);
                    break;
                }
                paren_depth -= 1;
            }
            _ => {}
        }
    }

    let open_pos = last_open_paren_pos?;

    // Extract the function name before `(` (identifier chars, plus `::` for `box::use` style)
    let before_paren = before_cursor[..open_pos].trim_end();
    let func_name = before_paren
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_' && c != '.' && c != ':')
        .next()?;

    if func_name.is_empty() {
        return None;
    }

    // Skip member access operators: obj$library( or env@require( are not real calls
    let func_start = before_paren.len() - func_name.len();
    if func_start > 0 {
        let preceding_char = before_paren[..func_start].chars().next_back();
        if matches!(preceding_char, Some('$' | '@')) {
            return None;
        }
    }

    // Check if it matches any configured function name
    if !func_names.iter().any(|f| f == func_name) {
        return None;
    }

    // Extract what's after the `(`
    let after_paren = &before_cursor[open_pos + 1..];

    // Skip if comma present (not first argument)
    if after_paren.contains(',') {
        return None;
    }

    let trimmed = after_paren.trim_start();

    // Skip if starts with a quote (string argument)
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return None;
    }

    // Extract the partial identifier
    let partial: String = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
        .collect();

    // Calculate start_pos: the byte position where the partial starts
    let whitespace_len = after_paren.len() - trimmed.len();
    let start_pos = open_pos + 1 + whitespace_len;

    Some(LibraryContext { partial, start_pos })
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
            namespace_cache: HashMap::new(),
            namespace_fuzzy_cache: None,
        }
    }

    /// Create a new RCompleter with all settings including fuzzy namespace.
    pub fn with_settings_full(
        timeout_ms: u64,
        debounce_ms: u64,
        auto_paren_limit: usize,
        fuzzy_namespace: bool,
        package_functions: Vec<String>,
    ) -> Self {
        RCompleter {
            timeout_ms,
            debounce_ms,
            auto_paren_limit,
            cache: None,
            fuzzy_namespace,
            package_functions,
            namespace_cache: HashMap::new(),
            namespace_fuzzy_cache: None,
        }
    }

    /// Check if the new token extends the cached token (prefix extension).
    fn is_prefix_extension(&self, new_token: &str) -> bool {
        if let Some(cache) = &self.cache {
            // New token must start with the cached token
            // e.g., "pri" extends "pr", but "po" does not
            new_token.starts_with(&cache.token) && new_token != cache.token
        } else {
            false
        }
    }

    /// Check if we should use cached results (within debounce window, same token).
    fn should_use_cache(&self, token: &str) -> bool {
        if let Some(cache) = &self.cache {
            // Don't use empty cache
            if cache.completions.is_empty() {
                return false;
            }

            // Don't use cache for empty tokens (context likely changed)
            if token.is_empty() || cache.token.is_empty() {
                return false;
            }

            // Don't use cache for package:: completions (always fetch fresh, worth waiting)
            if token.contains("::") || cache.token.contains("::") {
                return false;
            }

            // Use cache if:
            // 1. Same token and within debounce window
            // 2. Token is a prefix extension of cached token
            if cache.token == token {
                cache.timestamp.elapsed() < Duration::from_millis(self.debounce_ms)
            } else {
                self.is_prefix_extension(token)
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
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
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

        // Try to use cache for prefix extensions or debounced requests
        let completions = if self.should_use_cache(&token) {
            // Use cached results, filtering for the current token
            self.filter_cached(&token)
        } else {
            // Fetch fresh completions from R
            let fresh = match arf_harp::completion::get_completions(line, pos, self.timeout_ms) {
                Ok(c) => c,
                Err(_) => {
                    // On error, invalidate cache and return empty
                    self.invalidate_cache();
                    return vec![];
                }
            };

            // Only cache non-empty results with non-empty tokens
            if !fresh.is_empty() && !token.is_empty() {
                self.cache = Some(CompletionCache {
                    token: token.clone(),
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

    /// Cache duration for namespace exports (5 minutes).
    const NAMESPACE_CACHE_DURATION: Duration = Duration::from_secs(300);

    /// Build the cache key for a package namespace lookup.
    fn namespace_cache_key(pkg: &str, triple_colon: bool) -> String {
        if triple_colon {
            format!("{}:::", pkg)
        } else {
            format!("{}::", pkg)
        }
    }

    /// Check whether the namespace fuzzy cache has a valid hit for the given input and position.
    fn is_namespace_fuzzy_cache_hit(&self, input: &str, start_pos: usize) -> bool {
        if let Some(cache) = &self.namespace_fuzzy_cache {
            cache.input == input
                && cache.start_pos == start_pos
                && cache.timestamp.elapsed() < Duration::from_millis(self.debounce_ms)
        } else {
            false
        }
    }

    /// Store namespace exports in the cache.
    ///
    /// Empty results are not cached so that completions recover immediately
    /// once a package becomes available. Any previously cached (now-stale)
    /// entry for the same key is removed in that case.
    ///
    /// Expired entries for other packages are evicted on each insert.
    fn store_namespace_exports(&mut self, pkg: &str, triple_colon: bool, exports: Vec<String>) {
        let cache_key = Self::namespace_cache_key(pkg, triple_colon);

        if exports.is_empty() {
            // Remove any existing entry — this handles the case where a package
            // was previously available (cached with non-empty exports) but has
            // since been unloaded or removed mid-session. Without this, stale
            // non-empty results would persist until TTL expiry.
            self.namespace_cache.remove(&cache_key);
            return;
        }

        // Evict expired entries before inserting
        let ttl = Self::NAMESPACE_CACHE_DURATION;
        self.namespace_cache
            .retain(|_, v| v.timestamp.elapsed() < ttl);

        self.namespace_cache.insert(
            cache_key,
            NamespaceExportCache {
                exports,
                timestamp: Instant::now(),
            },
        );
    }

    /// Ensure exports for a package are cached, fetching from R if needed.
    ///
    /// Cache key includes `::` vs `:::` distinction since they return
    /// different sets of names (exported-only vs all namespace objects).
    fn ensure_namespace_cached(&mut self, pkg: &str, triple_colon: bool) {
        let cache_key = Self::namespace_cache_key(pkg, triple_colon);

        // Check cache
        if let Some(entry) = self.namespace_cache.get(&cache_key)
            && entry.timestamp.elapsed() < Self::NAMESPACE_CACHE_DURATION
        {
            return;
        }

        // Fetch from R
        let exports =
            arf_harp::completion::get_namespace_exports(pkg, triple_colon).unwrap_or_default();

        self.store_namespace_exports(pkg, triple_colon, exports);
    }

    /// Complete `pkg::partial` using fuzzy matching against namespace exports.
    fn complete_namespace_fuzzy(
        &mut self,
        ns_token: &NamespaceToken,
        pos: usize,
    ) -> Vec<Suggestion> {
        // Ensure exports are cached, then borrow to avoid cloning the full list
        self.ensure_namespace_cached(&ns_token.package, ns_token.triple_colon);
        let cache_key = Self::namespace_cache_key(&ns_token.package, ns_token.triple_colon);
        let exports = match self.namespace_cache.get(&cache_key) {
            Some(entry) if !entry.exports.is_empty() => &entry.exports,
            _ => return vec![],
        };

        let colons = if ns_token.triple_colon { ":::" } else { "::" };
        let prefix = format!("{}{}", ns_token.package, colons);
        let prefix_len = prefix.len();

        // Match exports against partial
        let matched: Vec<(String, Option<Vec<usize>>, u32)> = if ns_token.partial.is_empty() {
            // Empty partial: return all exports, sorted alphabetically
            let mut all: Vec<_> = exports.iter().map(|e| (e.clone(), None, 0u32)).collect();
            all.sort_by(|a, b| a.0.cmp(&b.0));
            all
        } else {
            // Fuzzy match against partial (only clone matching exports)
            let mut results: Vec<_> = exports
                .iter()
                .filter_map(|export| {
                    fuzzy_match(&ns_token.partial, export).map(|m| {
                        // Offset indices by prefix length (pkg::)
                        let indices: Vec<usize> =
                            m.indices.iter().map(|i| i + prefix_len).collect();
                        (export.clone(), Some(indices), m.score)
                    })
                })
                .collect();
            // Sort by score descending, then export name ascending for deterministic ties
            results.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
            results
        };

        if matched.is_empty() {
            return vec![];
        }

        // Filter out names containing backticks — R does not support escaping
        // backticks inside backtick-quoted identifiers, so such names cannot
        // be represented as valid R syntax in completions.
        let matched: Vec<_> = matched
            .into_iter()
            .filter(|(export, _, _)| !export.contains('`'))
            .collect();

        if matched.is_empty() {
            return vec![];
        }

        // Build qualified names with backtick quoting for non-syntactic names
        let qualified: Vec<(String, Option<Vec<usize>>)> = matched
            .iter()
            .map(|(export, indices, _)| {
                if needs_backtick_quoting(export) {
                    // Backtick-quoted: pkg::`name`
                    let value = format!("{}`{}`", prefix, export);
                    // Offset indices: prefix_len + 1 (for opening backtick)
                    let adjusted = indices
                        .as_ref()
                        .map(|idxs| idxs.iter().map(|&i| i + 1).collect());
                    (value, adjusted)
                } else {
                    let value = format!("{}{}", prefix, export);
                    (value, indices.clone())
                }
            })
            .collect();

        // Check function types for auto-paren (using qualified names)
        let check_names: Vec<&str> = qualified
            .iter()
            .take(self.auto_paren_limit)
            .map(|(name, _)| name.as_str())
            .collect();
        let is_function =
            arf_harp::completion::check_if_functions(&check_names).unwrap_or_default();

        let span = Span {
            start: ns_token.start_pos,
            end: pos,
        };

        qualified
            .into_iter()
            .zip(matched.iter())
            .enumerate()
            .map(|(i, ((base_value, indices), (export, _, _)))| {
                let is_func = is_function.get(i).copied().unwrap_or(false);

                let (value, extra_info) = if is_func && !has_special_suffix(export) {
                    (format!("{}()", base_value), Some("function".to_string()))
                } else {
                    (base_value, None)
                };

                Suggestion {
                    value,
                    display_override: None,
                    description: extra_info,
                    extra: None,
                    span,
                    append_whitespace: false,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect()
    }

    /// Complete package names inside `library()`, `require()`, or user-configured functions.
    fn complete_library_fuzzy(&self, lib_ctx: &LibraryContext, pos: usize) -> Vec<Suggestion> {
        let packages = match arf_harp::completion::get_installed_packages() {
            Ok(pkgs) => pkgs,
            Err(_) => return vec![],
        };

        let span = Span {
            start: lib_ctx.start_pos,
            end: pos,
        };

        if lib_ctx.partial.is_empty() {
            // Empty partial: return all packages sorted alphabetically
            let mut sorted = packages;
            sorted.sort();
            return sorted
                .into_iter()
                .map(|pkg| Suggestion {
                    value: pkg,
                    display_override: None,
                    description: None,
                    extra: None,
                    span,
                    append_whitespace: false,
                    style: None,
                    match_indices: None,
                })
                .collect();
        }

        // Fuzzy match against partial
        let mut results: Vec<_> = packages
            .iter()
            .filter_map(|pkg| {
                fuzzy_match(&lib_ctx.partial, pkg).map(|m| (pkg.clone(), m.indices, m.score))
            })
            .collect();

        // Sort by score descending, then name ascending for deterministic ties
        results.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

        results
            .into_iter()
            .map(|(pkg, indices, _)| Suggestion {
                value: pkg,
                display_override: None,
                description: None,
                extra: None,
                span,
                append_whitespace: false,
                style: None,
                match_indices: Some(indices),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
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

        // Insert an already-expired entry
        completer.namespace_cache.insert(
            "old_pkg::".to_string(),
            NamespaceExportCache {
                exports: vec!["old_func".to_string()],
                timestamp: Instant::now()
                    - (RCompleter::NAMESPACE_CACHE_DURATION + Duration::from_secs(1)),
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
}
