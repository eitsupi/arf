//! Shared cache of R's library search paths.

use crate::error::{HarpError, HarpResult};
use crate::eval_string_in_base;
use arf_libr::{SexpType, r_library};
use std::collections::HashSet;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct LibPathsCache {
    paths: Vec<String>,
    last_success: Option<Instant>,
}

impl LibPathsCache {
    const fn new() -> Self {
        Self {
            paths: Vec::new(),
            last_success: None,
        }
    }

    fn is_fresh(&self) -> bool {
        self.last_success
            .is_some_and(|last_success| last_success.elapsed() < CACHE_DURATION)
    }

    fn apply_refresh(&mut self, result: HarpResult<Vec<String>>) -> HarpResult<()> {
        let paths = result?;
        self.paths = paths;
        self.last_success = Some(Instant::now());
        Ok(())
    }
}

static LIB_PATHS_CACHE: Mutex<LibPathsCache> = Mutex::new(LibPathsCache::new());

const CACHE_DURATION: Duration = Duration::from_secs(300);

/// Refreshes the cached library paths if the cache is stale or empty.
///
/// A failed refresh leaves both the previous paths and `last_success` intact,
/// so the next call retries instead of treating the failed attempt as fresh.
pub fn populate_lib_paths() -> HarpResult<()> {
    let mut cache = LIB_PATHS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.is_fresh() {
        return Ok(());
    }

    let result = eval_string_in_base("invisible(.libPaths())")?;
    let paths = extract_paths(&result)?;
    cache.apply_refresh(Ok(paths))
}

/// Returns the current library paths, refreshing the cache when necessary.
pub fn lib_paths() -> HarpResult<Vec<String>> {
    populate_lib_paths()?;
    Ok(cached_lib_paths())
}

/// Returns the cached library paths without evaluating R.
pub fn cached_lib_paths() -> Vec<String> {
    LIB_PATHS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .paths
        .clone()
}

/// Find installed package directories in library-path order.
pub(crate) fn installed_package_dirs(paths: &[String]) -> Vec<(String, PathBuf)> {
    let mut seen = HashSet::new();
    let mut packages = Vec::new();

    for lib_path in paths {
        let Ok(entries) = Path::new(lib_path).read_dir() else {
            continue;
        };

        let mut names = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect::<Vec<_>>();
        names.sort_unstable();

        for name in names {
            let package_dir = Path::new(lib_path).join(&name);
            if package_dir.join("Meta").join("package.rds").exists() && seen.insert(name.clone()) {
                packages.push((name, package_dir));
            }
        }
    }

    packages
}

fn extract_paths(result: &crate::RObject) -> HarpResult<Vec<String>> {
    let lib = r_library()?;
    let sexp = result.sexp();

    if result.sexp_type()? != SexpType::StrSxp {
        return Err(HarpError::TypeMismatch {
            expected: "character vector".to_string(),
            actual: "non-character vector".to_string(),
        });
    }

    let length = unsafe { (lib.rf_length)(sexp) } as isize;
    let mut paths = Vec::with_capacity(length as usize);
    for index in 0..length {
        let path = unsafe {
            let element = (lib.string_elt)(sexp, index);
            let chars = (lib.r_charsxp)(element);
            if chars.is_null() {
                continue;
            }
            CStr::from_ptr(chars).to_string_lossy().into_owned()
        };
        paths.push(path);
    }

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_refresh_state_preserves_good_data_on_failure() {
        let mut cache = LibPathsCache {
            paths: vec!["/old/library".to_string()],
            last_success: Some(Instant::now()),
        };

        assert!(cache.is_fresh());
        let last_success = cache.last_success;
        let error = HarpError::TypeMismatch {
            expected: "character vector".to_string(),
            actual: "numeric vector".to_string(),
        };
        assert!(cache.apply_refresh(Err(error)).is_err());
        assert_eq!(cache.paths, ["/old/library"]);
        assert_eq!(cache.last_success, last_success);
        assert!(cache.is_fresh());

        cache.last_success = Some(Instant::now() - CACHE_DURATION);
        assert!(!cache.is_fresh());
        cache
            .apply_refresh(Ok(vec!["/new/library".to_string()]))
            .unwrap();
        assert_eq!(cache.paths, ["/new/library"]);
        assert!(cache.is_fresh());
    }
}
