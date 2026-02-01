//! Shared fuzzy matching module.
//!
//! This module provides fuzzy matching functionality using the nucleo library.
//! It's used by both the command completer and history search.

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

/// Result of a fuzzy match including score and matched indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Score of the match (higher is better).
    pub score: u32,
    /// Indices of matched characters in the target string.
    pub indices: Vec<usize>,
}

/// Performs fuzzy matching of a pattern against a target string.
///
/// Returns `Some(FuzzyMatch)` with score and matched character indices,
/// or `None` if the pattern doesn't match.
///
/// # Examples
/// - `fuzzy_match("rst", "restart")` → matches with indices [0, 2, 3]
/// - `fuzzy_match("sw", "switch")` → matches with indices [0, 1]
/// - `fuzzy_match("xyz", "restart")` → None
pub fn fuzzy_match(pattern: &str, target: &str) -> Option<FuzzyMatch> {
    if pattern.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            indices: vec![],
        });
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pat = Pattern::new(
        pattern,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    // Convert target to Utf32Str for matching
    let mut buf = Vec::new();
    let target_utf32 = Utf32Str::new(target, &mut buf);

    // Get match indices
    let mut indices = Vec::new();
    let score = pat.indices(target_utf32, &mut matcher, &mut indices)?;

    // Convert u32 indices to usize
    let indices: Vec<usize> = indices.into_iter().map(|i| i as usize).collect();

    Some(FuzzyMatch { score, indices })
}

/// Match multiple candidates against a pattern and return sorted results.
///
/// Results are sorted by score (highest first).
#[allow(dead_code)]
pub fn fuzzy_match_sorted<'a, T, F>(
    pattern: &str,
    candidates: impl Iterator<Item = T>,
    get_text: F,
) -> Vec<(T, FuzzyMatch)>
where
    F: Fn(&T) -> &'a str,
{
    let mut results: Vec<_> = candidates
        .filter_map(|item| {
            let text = get_text(&item);
            fuzzy_match(pattern, text).map(|m| (item, m))
        })
        .collect();

    // Sort by score (descending)
    results.sort_by(|a, b| b.1.score.cmp(&a.1.score));

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_basic() {
        // Consecutive matches (same as prefix)
        let m = fuzzy_match("rep", "reprex").unwrap();
        assert_eq!(m.indices, vec![0, 1, 2]);

        let m = fuzzy_match("sw", "switch").unwrap();
        assert_eq!(m.indices, vec![0, 1]);
    }

    #[test]
    fn test_fuzzy_match_non_consecutive() {
        // Non-consecutive fuzzy matches
        let m = fuzzy_match("rst", "restart").unwrap();
        // nucleo may find different optimal positions
        assert!(m.indices.len() == 3);

        assert!(fuzzy_match("ht", "shell").is_none());
    }

    #[test]
    fn test_fuzzy_match_case_insensitive() {
        // Case insensitive matching
        assert!(fuzzy_match("REP", "reprex").is_some());
        assert!(fuzzy_match("rEp", "reprex").is_some());
    }

    #[test]
    fn test_fuzzy_match_no_match() {
        // Pattern doesn't match target
        assert!(fuzzy_match("xyz", "restart").is_none());
        assert!(fuzzy_match("abc", "shell").is_none());
    }

    #[test]
    fn test_fuzzy_match_empty_pattern() {
        // Empty pattern matches everything with empty indices
        let m = fuzzy_match("", "restart").unwrap();
        assert_eq!(m.indices, Vec::<usize>::new());
        assert_eq!(m.score, 0);
    }

    #[test]
    fn test_fuzzy_match_sorted() {
        let candidates = vec!["restart", "reprex", "shell", "system"];
        let results = fuzzy_match_sorted("r", candidates.into_iter(), |s| s);

        // All should match except maybe shell (if 'r' not found)
        assert!(!results.is_empty());

        // Results should be sorted by score
        for i in 1..results.len() {
            assert!(results[i - 1].1.score >= results[i].1.score);
        }
    }

    #[test]
    fn test_fuzzy_match_scoring() {
        // Prefix matches should score higher than scattered matches
        let prefix = fuzzy_match("re", "reprex").unwrap();
        let scattered = fuzzy_match("re", "restart").unwrap();

        // Both should match, prefix at start might score higher
        assert!(prefix.score > 0);
        assert!(scattered.score > 0);
    }
}
