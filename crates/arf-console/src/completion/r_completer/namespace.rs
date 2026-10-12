//! Namespace completion: `pkg::partial` parsing, the export and debounce
//! caches, and fuzzy completion of namespace members and package names.

use super::RCompleter;
use super::context::LibraryContext;
use super::quoting::{has_special_suffix, needs_backtick_quoting};
use crate::fuzzy::fuzzy_match;
use reedline::{Span, Suggestion};
use std::time::{Duration, Instant};

/// Parsed `pkg::partial` or `pkg:::partial` token from user input.
#[derive(Debug, PartialEq)]
pub(super) struct NamespaceToken {
    /// Package name (e.g., "sf").
    pub(super) package: String,
    /// Partial function name after `::` (e.g., "geo"). May be empty.
    pub(super) partial: String,
    /// Whether `:::` was used (internal access).
    pub(super) triple_colon: bool,
    /// Byte position in the line where the token starts (beginning of `pkg`).
    pub(super) start_pos: usize,
}

/// Parse a `pkg::partial` or `pkg:::partial` pattern from the text before the cursor.
///
/// Returns `None` if no namespace pattern is found.
pub(super) fn parse_namespace_token(line: &str, cursor_pos: usize) -> Option<NamespaceToken> {
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
    } else {
        let rest = before_partial.strip_suffix("::")?;
        (false, rest)
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
pub(super) struct NamespaceExportCache {
    pub(super) exports: Vec<String>,
    pub(super) timestamp: Instant,
}

/// Cache for fuzzy namespace completion results (debounce).
pub(super) struct NamespaceFuzzyCache {
    /// The full input used for this cache (e.g., "pkg::partial").
    pub(super) input: String,
    /// Start position of the namespace token in the line.
    pub(super) start_pos: usize,
    /// Cached suggestions.
    pub(super) suggestions: Vec<Suggestion>,
    /// When the cache was created.
    pub(super) timestamp: Instant,
}

impl RCompleter {
    /// Cache duration for namespace exports (5 minutes).
    pub(super) const NAMESPACE_CACHE_DURATION: Duration = Duration::from_secs(300);

    /// Build the cache key for a package namespace lookup.
    fn namespace_cache_key(pkg: &str, triple_colon: bool) -> String {
        if triple_colon {
            format!("{}:::", pkg)
        } else {
            format!("{}::", pkg)
        }
    }

    /// Check whether the namespace fuzzy cache has a valid hit for the given input and position.
    pub(super) fn is_namespace_fuzzy_cache_hit(&self, input: &str, start_pos: usize) -> bool {
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
    pub(super) fn store_namespace_exports(
        &mut self,
        pkg: &str,
        triple_colon: bool,
        exports: Vec<String>,
    ) {
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
    pub(super) fn complete_namespace_fuzzy(
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
    pub(super) fn complete_library_fuzzy(
        &self,
        lib_ctx: &LibraryContext,
        pos: usize,
    ) -> Vec<Suggestion> {
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
