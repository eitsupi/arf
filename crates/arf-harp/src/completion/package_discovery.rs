//! Installed package discovery and caching.

use crate::error::HarpResult;
use crate::lib_paths::{installed_package_dirs, lib_paths};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Cache for installed packages.
struct PackageCache {
    packages: Vec<String>,
    last_updated: Option<Instant>,
}

impl PackageCache {
    const fn new() -> Self {
        PackageCache {
            packages: Vec::new(),
            last_updated: None,
        }
    }

    fn is_fresh(&self) -> bool {
        self.last_updated
            .is_some_and(|last_updated| last_updated.elapsed() < CACHE_DURATION)
    }
}

static PACKAGE_CACHE: Mutex<PackageCache> = Mutex::new(PackageCache::new());

/// Cache duration for installed packages (5 minutes).
const CACHE_DURATION: Duration = Duration::from_secs(300);

/// Get the list of installed packages with caching.
///
/// The cache is checked before refreshing R's library paths, so completion
/// avoids R evaluation while the five-minute cache is fresh. Consequently,
/// package and library-path changes may remain undetected for up to five
/// minutes on this completion path.
pub fn get_installed_packages() -> HarpResult<Vec<String>> {
    let cache = PACKAGE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.is_fresh() {
        return Ok(cache.packages.clone());
    }
    drop(cache);

    let paths = lib_paths()?;

    // Once the TTL has elapsed, always rescan, even if the library paths are unchanged.
    let packages = scan_installed_packages(&paths);

    // Update cache
    let mut cache = PACKAGE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.packages = packages.clone();
    cache.last_updated = Some(Instant::now());

    Ok(packages)
}

/// Get package completions for a partial package name (for library()/require()).
pub(super) fn get_package_completions(partial: &str) -> HarpResult<Vec<String>> {
    let packages = get_installed_packages()?;

    let completions: Vec<String> = packages
        .into_iter()
        .filter(|pkg| pkg.starts_with(partial))
        .collect();

    Ok(completions)
}

/// Get package completions with `::` suffix (for namespace access).
pub(super) fn get_namespace_completions(partial: &str) -> HarpResult<Vec<String>> {
    let packages = get_installed_packages()?;

    let completions: Vec<String> = packages
        .into_iter()
        .filter(|pkg| pkg.starts_with(partial))
        .map(|pkg| format!("{}::", pkg))
        .collect();

    Ok(completions)
}

/// Find package directories by checking their metadata marker files.
fn scan_installed_packages(paths: &[String]) -> Vec<String> {
    installed_package_dirs(paths)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_cache_freshness_is_time_based() {
        let mut cache = PackageCache {
            packages: vec!["old-package".to_string()],
            last_updated: Some(Instant::now()),
        };
        assert!(cache.is_fresh());

        cache.last_updated = Some(Instant::now() - CACHE_DURATION);
        assert!(!cache.is_fresh());
    }

    #[test]
    fn test_scan_installed_packages() {
        let temp_dir = tempfile::tempdir().expect("temporary directory should be created");
        let lib_one = temp_dir.path().join("library-one");
        let lib_two = temp_dir.path().join("library-two");

        std::fs::create_dir_all(lib_one.join("pkgA/Meta")).unwrap();
        std::fs::write(lib_one.join("pkgA/Meta/package.rds"), []).unwrap();
        std::fs::create_dir_all(lib_one.join("pkgB")).unwrap();
        std::fs::create_dir_all(lib_one.join(".hidden/Meta")).unwrap();
        std::fs::write(lib_one.join(".hidden/Meta/package.rds"), []).unwrap();

        std::fs::create_dir_all(lib_two.join("pkgA/Meta")).unwrap();
        std::fs::write(lib_two.join("pkgA/Meta/package.rds"), []).unwrap();
        std::fs::create_dir_all(lib_two.join("pkgC/Meta")).unwrap();
        std::fs::write(lib_two.join("pkgC/Meta/package.rds"), []).unwrap();

        let paths = vec![
            lib_one.to_string_lossy().into_owned(),
            lib_two.to_string_lossy().into_owned(),
        ];
        assert_eq!(scan_installed_packages(&paths), ["pkgA", "pkgC"]);
    }
}
