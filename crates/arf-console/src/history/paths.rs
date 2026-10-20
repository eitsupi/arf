//! Filesystem path helpers shared by history location handling.

use std::fs;
use std::path::Path;

/// Whether two paths name the same directory, also through symlinks or a
/// different spelling. Startup and `arf history migrate` both use this, so a
/// legacy location that is the current one never produces a warning that
/// migrate cannot resolve.
pub(crate) fn same_directory(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (fs::canonicalize(a), fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

#[cfg(test)]
mod tests {
    use super::same_directory;

    #[test]
    fn same_directory_sees_through_spelling_but_not_across_directories() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        assert!(same_directory(&a, &a.join(".")));
        assert!(!same_directory(&a, &b));
        // Paths that do not exist yet are only the same when spelled alike.
        let missing = root.path().join("missing");
        assert!(same_directory(&missing, &missing));
        assert!(!same_directory(&missing, &root.path().join("other")));
    }

    #[cfg(unix)]
    #[test]
    fn same_directory_follows_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let link = root.path().join("link");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(same_directory(&link, &target));
    }
}
