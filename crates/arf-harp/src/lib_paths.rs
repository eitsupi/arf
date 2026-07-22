//! Shared cache of R's library search paths.

use crate::error::{HarpError, HarpResult};
use crate::eval_string_in_base;
use arf_libr::{SexpType, r_library};
use std::collections::HashSet;
use std::ffi::CStr;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

struct LibPathsCache {
    paths: Vec<String>,
}

impl LibPathsCache {
    const fn new() -> Self {
        Self { paths: Vec::new() }
    }

    fn apply_refresh(&mut self, result: HarpResult<Vec<String>>) -> HarpResult<()> {
        let paths = result?;
        self.paths = paths;
        Ok(())
    }
}

static LIB_PATHS_CACHE: Mutex<LibPathsCache> = Mutex::new(LibPathsCache::new());

/// Refreshes the cached library paths.
///
/// A failed refresh leaves the previous paths intact.
pub fn populate_lib_paths() -> HarpResult<()> {
    let result = eval_string_in_base("invisible(.libPaths())")?;
    let paths = extract_paths(&result)?;

    let mut cache = LIB_PATHS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.apply_refresh(Ok(paths))
}

/// Returns the current library paths, refreshing the cache first.
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
            if is_installed_package_dir(&package_dir) && seen.insert(name.clone()) {
                packages.push((name, package_dir));
            }
        }
    }

    packages
}

fn is_installed_package_dir(dir: &Path) -> bool {
    dir.join("Meta").join("package.rds").exists()
}

/// Find one installed package directory without enumerating library contents.
pub(crate) fn installed_package_dir(paths: &[String], package: &str) -> Option<PathBuf> {
    let mut components = Path::new(package).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return None;
    }

    paths
        .iter()
        .map(|lib_path| Path::new(lib_path).join(package))
        .find(|dir| is_installed_package_dir(dir))
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
        };

        let error = HarpError::TypeMismatch {
            expected: "character vector".to_string(),
            actual: "numeric vector".to_string(),
        };
        assert!(cache.apply_refresh(Err(error)).is_err());
        assert_eq!(cache.paths, ["/old/library"]);
        cache
            .apply_refresh(Ok(vec!["/new/library".to_string()]))
            .unwrap();
        assert_eq!(cache.paths, ["/new/library"]);
    }

    #[test]
    fn single_package_lookup_checks_paths_in_order_and_rejects_traversal() {
        let temp_dir = tempfile::tempdir().expect("temporary directory should be created");
        let first = temp_dir.path().join("first");
        let second = temp_dir.path().join("second");
        std::fs::create_dir_all(first.join("pkg/Meta")).unwrap();
        std::fs::write(first.join("pkg/Meta/package.rds"), []).unwrap();
        std::fs::create_dir_all(second.join("pkg/Meta")).unwrap();
        std::fs::write(second.join("pkg/Meta/package.rds"), []).unwrap();

        let paths = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        assert_eq!(
            installed_package_dir(&paths, "pkg"),
            Some(first.join("pkg"))
        );
        assert_eq!(installed_package_dir(&paths, "../pkg"), None);
        assert_eq!(installed_package_dir(&paths, r"pkg\nested"), None);
    }

    #[cfg(windows)]
    #[test]
    fn single_package_lookup_rejects_drive_relative_paths() {
        assert_eq!(installed_package_dir(&[], "C:pkg"), None);
    }
}
