//! Shared cache of R's library search paths.

use crate::error::{HarpError, HarpResult};
use crate::eval_string;
use arf_libr::{SexpType, r_library};
use std::collections::HashSet;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct LibPathsCache {
    paths: Vec<String>,
    populated: bool,
}

impl LibPathsCache {
    const fn new() -> Self {
        Self {
            paths: Vec::new(),
            populated: false,
        }
    }
}

static LIB_PATHS_CACHE: Mutex<LibPathsCache> = Mutex::new(LibPathsCache::new());

/// Evaluate R's `.libPaths()` once and store the resulting paths.
pub fn populate_lib_paths() -> HarpResult<()> {
    let mut cache = LIB_PATHS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.populated {
        return Ok(());
    }

    cache.populated = true;
    let result = eval_string("tryCatch(invisible(.libPaths()), error = function(e) character(0))");
    match result {
        Ok(result) => {
            cache.paths = extract_paths(&result)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Return the cached R library paths, or an empty vector before population or
/// after a failed population attempt.
pub fn lib_paths() -> Vec<String> {
    LIB_PATHS_CACHE
        .lock()
        .map(|cache| cache.paths.clone())
        .unwrap_or_default()
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
