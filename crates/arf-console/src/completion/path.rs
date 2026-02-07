//! Rust-native path completion with fuzzy matching.
//!
//! This module provides file/directory path completion using fuzzy matching,
//! inspired by nushell's approach. It replaces R-based path completion for
//! better UX and performance.

use crate::fuzzy::fuzzy_match;
use std::path::Path;

/// Normalize path separators to forward slashes.
/// This is needed for R compatibility on Windows, since backslashes
/// are escape characters in R strings.
#[cfg(windows)]
pub fn normalize_separators(path: &str) -> String {
    path.replace('\\', "/")
}

/// On non-Windows platforms, no normalization is needed.
#[cfg(not(windows))]
pub fn normalize_separators(path: &str) -> String {
    path.to_string()
}

/// Result of a path completion.
#[derive(Debug, Clone)]
pub struct PathCompletion {
    /// The completed path string.
    pub path: String,
    /// Whether this is a directory.
    pub is_dir: bool,
    /// Match indices for highlighting (if fuzzy matched).
    pub match_indices: Option<Vec<usize>>,
    /// Fuzzy match score (higher is better).
    pub score: u32,
}

/// Options for path completion.
#[derive(Debug, Clone)]
pub struct PathCompletionOptions {
    /// Use fuzzy matching instead of prefix matching.
    pub fuzzy: bool,
    /// Show hidden files (starting with `.`).
    pub show_hidden: bool,
    /// Only complete directories (not files).
    pub directories_only: bool,
}

impl Default for PathCompletionOptions {
    fn default() -> Self {
        Self {
            fuzzy: true,
            show_hidden: true,
            directories_only: false,
        }
    }
}

/// Expand tilde to home directory.
/// Returns normalized path with forward slashes for R compatibility.
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with('~')
        && let Some(home) = dirs::home_dir()
    {
        // Normalize home path to use forward slashes (important for Windows)
        let home_str = normalize_separators(&home.to_string_lossy());
        if path == "~" {
            return home_str;
        } else if let Some(rest) = path.strip_prefix("~/") {
            return format!("{}/{}", home_str, rest);
        }
    }
    // Also normalize input path in case it contains backslashes
    normalize_separators(path)
}

/// Split a path into directory and partial filename components.
/// Returns normalized paths with forward slashes for R compatibility.
fn split_path(path: &str) -> (String, String) {
    let expanded = expand_tilde(path);
    let path_obj = Path::new(&expanded);

    // Handle the case where path ends with separator
    if expanded.ends_with('/') || expanded.ends_with('\\') {
        // Normalize to forward slashes for R compatibility
        return (normalize_separators(&expanded), String::new());
    }

    // Get parent directory and filename
    let parent = path_obj
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let filename = path_obj
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Preserve original prefix for display
    let display_parent = if path.starts_with('~') && !parent.starts_with('~') {
        // Reconstruct with tilde
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy();
            if parent.starts_with(home_str.as_ref()) {
                format!("~{}", &parent[home_str.len()..])
            } else {
                parent
            }
        } else {
            parent
        }
    } else {
        parent
    };

    // Normalize to forward slashes for R compatibility
    (normalize_separators(&display_parent), filename)
}

/// List entries in a directory.
fn list_directory(dir: &Path) -> Vec<(String, bool)> {
    let mut entries = Vec::new();

    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push((name, is_dir));
        }
    }

    entries
}

/// Complete a path with fuzzy matching.
///
/// # Arguments
/// * `partial` - The partial path to complete (may include directory components)
/// * `cwd` - Current working directory for relative paths
/// * `options` - Completion options
///
/// # Returns
/// A vector of path completions sorted by relevance.
pub fn complete_path(
    partial: &str,
    cwd: &Path,
    options: &PathCompletionOptions,
) -> Vec<PathCompletion> {
    let (dir_part, file_part) = split_path(partial);

    // Determine the directory to search in
    let search_dir = if dir_part.is_empty() {
        cwd.to_path_buf()
    } else {
        let expanded_dir = expand_tilde(&dir_part);
        let dir_path = Path::new(&expanded_dir);
        if dir_path.is_absolute() {
            dir_path.to_path_buf()
        } else {
            cwd.join(dir_path)
        }
    };

    // List directory entries
    let entries = list_directory(&search_dir);

    // Filter and match entries
    let mut completions: Vec<PathCompletion> = Vec::new();
    let mut hidden_completions: Vec<PathCompletion> = Vec::new();

    for (name, is_dir) in entries {
        // Skip non-directories if directories_only
        if options.directories_only && !is_dir {
            continue;
        }

        let is_hidden = name.starts_with('.');

        // Skip hidden files unless explicitly searching for them
        if is_hidden && !options.show_hidden && !file_part.starts_with('.') {
            continue;
        }

        // Match against the partial filename
        let (matches, score, match_indices) = if file_part.is_empty() {
            // Empty partial matches everything
            (true, 0, None)
        } else if options.fuzzy {
            // Fuzzy matching
            if let Some(m) = fuzzy_match(&file_part, &name) {
                (true, m.score, Some(m.indices))
            } else {
                (false, 0, None)
            }
        } else {
            // Prefix matching (case-insensitive)
            let matches = name.to_lowercase().starts_with(&file_part.to_lowercase());
            let indices = if matches {
                Some((0..file_part.len()).collect())
            } else {
                None
            };
            (matches, if matches { 100 } else { 0 }, indices)
        };

        if matches {
            // Build the completed path
            let completed_path = if dir_part.is_empty() {
                if is_dir {
                    format!("{}/", name)
                } else {
                    name.clone()
                }
            } else {
                let sep = if dir_part.ends_with('/') { "" } else { "/" };
                if is_dir {
                    format!("{}{}{}/", dir_part, sep, name)
                } else {
                    format!("{}{}{}", dir_part, sep, name)
                }
            };

            let completion = PathCompletion {
                path: completed_path,
                is_dir,
                match_indices,
                score,
            };

            if is_hidden {
                hidden_completions.push(completion);
            } else {
                completions.push(completion);
            }
        }
    }

    // Sort by score (descending), then alphabetically
    completions.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hidden_completions.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));

    // Append hidden files at the end
    completions.append(&mut hidden_completions);

    completions
}

/// Detect if a string looks like a path.
///
/// Returns true if the string:
/// - Starts with `/`, `./`, `../`, `~/`
/// - Contains path separators
/// - Looks like a relative path
#[allow(dead_code)]
pub fn looks_like_path(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    // Absolute or home-relative paths
    if s.starts_with('/') || s.starts_with("~/") || s.starts_with(r"~\") {
        return true;
    }

    // Relative paths
    if s.starts_with("./") || s.starts_with(r".\") {
        return true;
    }
    if s.starts_with("../") || s.starts_with(r"..\") {
        return true;
    }

    // Contains path separator - likely a path
    if s.contains('/') || s.contains('\\') {
        return true;
    }

    // Single dot or double dot
    if s == "." || s == ".." {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let base = dir.path();

        // Create test files and directories
        fs::create_dir(base.join("src")).unwrap();
        fs::create_dir(base.join("tests")).unwrap();
        fs::create_dir(base.join(".hidden")).unwrap();
        File::create(base.join("README.md")).unwrap();
        File::create(base.join("Cargo.toml")).unwrap();
        File::create(base.join(".gitignore")).unwrap();
        File::create(base.join("src/main.rs")).unwrap();
        File::create(base.join("src/lib.rs")).unwrap();

        dir
    }

    #[test]
    fn test_expand_tilde() {
        // Tilde with slash should expand
        let expanded = expand_tilde("~/Documents");
        if let Some(home) = dirs::home_dir() {
            // Compare with normalized home path (forward slashes)
            let home_normalized = normalize_separators(&home.to_string_lossy());
            assert!(expanded.starts_with(&home_normalized));
            assert!(expanded.ends_with("Documents"));
        }

        // Just tilde
        let expanded = expand_tilde("~");
        if let Some(home) = dirs::home_dir() {
            let home_normalized = normalize_separators(&home.to_string_lossy());
            assert_eq!(expanded, home_normalized);
        }

        // Relative path - unchanged on all platforms
        assert_eq!(expand_tilde("./foo"), "./foo");
    }

    #[test]
    #[cfg(unix)]
    fn test_expand_tilde_unix_paths() {
        // Unix absolute paths - unchanged
        assert_eq!(expand_tilde("/usr/bin"), "/usr/bin");
    }

    #[test]
    #[cfg(windows)]
    fn test_expand_tilde_windows_paths() {
        // Windows absolute paths - backslashes normalized to forward slashes
        assert_eq!(expand_tilde("C:\\Users"), "C:/Users");
    }

    #[test]
    fn test_split_path() {
        let (dir, file) = split_path("src/main.rs");
        assert_eq!(dir, "src");
        assert_eq!(file, "main.rs");

        let (dir, file) = split_path("main.rs");
        assert_eq!(dir, "");
        assert_eq!(file, "main.rs");

        let (dir, file) = split_path("src/");
        assert_eq!(dir, "src/");
        assert_eq!(file, "");

        let (dir, file) = split_path("/usr/bin/ls");
        assert_eq!(dir, "/usr/bin");
        assert_eq!(file, "ls");
    }

    #[test]
    fn test_complete_path_prefix() {
        let dir = setup_test_dir();
        let options = PathCompletionOptions {
            fuzzy: false,
            show_hidden: true,
            directories_only: false,
        };

        // Complete "R" -> should match README.md
        let completions = complete_path("R", dir.path(), &options);
        assert!(!completions.is_empty());
        assert!(completions.iter().any(|c| c.path == "README.md"));

        // Complete "sr" -> should match src/
        let completions = complete_path("sr", dir.path(), &options);
        assert!(completions.iter().any(|c| c.path == "src/"));
    }

    #[test]
    fn test_complete_path_fuzzy() {
        let dir = setup_test_dir();
        let options = PathCompletionOptions::default();

        // Fuzzy: "rdm" -> should match README.md
        let completions = complete_path("rdm", dir.path(), &options);
        assert!(
            completions.iter().any(|c| c.path == "README.md"),
            "Expected README.md in completions: {:?}",
            completions
        );

        // Fuzzy: "cgt" -> should match Cargo.toml
        let completions = complete_path("cgt", dir.path(), &options);
        assert!(
            completions.iter().any(|c| c.path == "Cargo.toml"),
            "Expected Cargo.toml in completions: {:?}",
            completions
        );
    }

    #[test]
    fn test_complete_path_nested() {
        let dir = setup_test_dir();
        let options = PathCompletionOptions::default();

        // Complete "src/m" -> should match src/main.rs
        let completions = complete_path("src/m", dir.path(), &options);
        assert!(
            completions.iter().any(|c| c.path == "src/main.rs"),
            "Expected src/main.rs in completions: {:?}",
            completions
        );

        // Complete "src/" -> should list all files in src/
        let completions = complete_path("src/", dir.path(), &options);
        assert!(completions.iter().any(|c| c.path == "src/main.rs"));
        assert!(completions.iter().any(|c| c.path == "src/lib.rs"));
    }

    #[test]
    fn test_complete_path_hidden_last() {
        let dir = setup_test_dir();
        let options = PathCompletionOptions::default();

        // List all files - hidden should be last
        let completions = complete_path("", dir.path(), &options);
        assert!(!completions.is_empty());

        // Find positions of hidden and non-hidden files
        let hidden_pos = completions
            .iter()
            .position(|c| c.path.starts_with('.'))
            .unwrap_or(usize::MAX);
        let non_hidden_pos = completions
            .iter()
            .position(|c| !c.path.starts_with('.'))
            .unwrap_or(usize::MAX);

        // Non-hidden should come before hidden
        if hidden_pos != usize::MAX && non_hidden_pos != usize::MAX {
            assert!(
                non_hidden_pos < hidden_pos,
                "Non-hidden files should come before hidden files"
            );
        }
    }

    #[test]
    fn test_complete_path_directories_only() {
        let dir = setup_test_dir();
        let options = PathCompletionOptions {
            directories_only: true,
            ..Default::default()
        };

        let completions = complete_path("", dir.path(), &options);

        // Should only have directories
        for c in &completions {
            assert!(c.is_dir, "Expected only directories, got: {}", c.path);
        }

        // Should have src/ and tests/
        assert!(completions.iter().any(|c| c.path == "src/"));
        assert!(completions.iter().any(|c| c.path == "tests/"));
    }

    #[test]
    fn test_looks_like_path() {
        // Should be paths
        assert!(looks_like_path("/usr/bin"));
        assert!(looks_like_path("./foo"));
        assert!(looks_like_path("../bar"));
        assert!(looks_like_path("~/Documents"));
        assert!(looks_like_path("src/main.rs"));
        assert!(looks_like_path("."));
        assert!(looks_like_path(".."));

        // Should not be paths
        assert!(!looks_like_path(""));
        assert!(!looks_like_path("print"));
        assert!(!looks_like_path("my_variable"));
        assert!(!looks_like_path("data.frame"));
    }

    #[test]
    #[cfg(windows)]
    fn test_normalize_separators_windows() {
        // Forward slashes should be preserved
        assert_eq!(normalize_separators("src/main.rs"), "src/main.rs");
        assert_eq!(normalize_separators("/usr/bin"), "/usr/bin");

        // Backslashes should be converted to forward slashes
        assert_eq!(normalize_separators(r"src\main.rs"), "src/main.rs");
        assert_eq!(normalize_separators(r"C:\Users\foo"), "C:/Users/foo");
        assert_eq!(
            normalize_separators(r"C:\Users\foo\bar\baz.txt"),
            "C:/Users/foo/bar/baz.txt"
        );

        // Mixed separators should all become forward slashes
        assert_eq!(
            normalize_separators(r"C:\Users/foo\bar"),
            "C:/Users/foo/bar"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_split_path_windows_backslashes() {
        // Windows-style paths should be normalized to forward slashes
        let (dir, file) = split_path(r"C:\Users\foo\bar.txt");
        assert_eq!(dir, "C:/Users/foo");
        assert_eq!(file, "bar.txt");

        let (dir, file) = split_path(r"C:\Users\");
        assert_eq!(dir, "C:/Users/");
        assert_eq!(file, "");
    }

    #[test]
    fn test_complete_path_no_backslashes() {
        // Verify that path completions never contain backslashes
        // This is important for R compatibility on Windows
        let dir = setup_test_dir();
        let options = PathCompletionOptions::default();

        let completions = complete_path("", dir.path(), &options);
        for c in &completions {
            assert!(
                !c.path.contains('\\'),
                "Path completion should not contain backslashes: {}",
                c.path
            );
        }

        // Also test nested paths
        let completions = complete_path("src/", dir.path(), &options);
        for c in &completions {
            assert!(
                !c.path.contains('\\'),
                "Nested path completion should not contain backslashes: {}",
                c.path
            );
        }
    }
}
