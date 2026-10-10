//! Preserve a provider's validated library instead of rediscovering another R.

use crate::{RError, RResult};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, PartialEq, Eq)]
struct Selection {
    library: PathBuf,
    dependency_prefix: Option<PathBuf>,
}

static SELECTION: OnceLock<Selection> = OnceLock::new();

/// Pin a validated R library before initialization. A dependency prefix enables
/// prefix-limited Windows DLL search; `None` retains the ordinary R policy.
/// Once selected, missing or unloadable libraries are errors, never fallback.
pub fn select_r_library(library: PathBuf, dependency_prefix: Option<PathBuf>) -> RResult<()> {
    let selection = Selection {
        library,
        dependency_prefix,
    };
    if let Some(previous) = SELECTION.get() {
        if previous == &selection {
            return Ok(());
        }
        return Err(RError::LibraryNotFound(
            "An R library has already been selected".into(),
        ));
    }
    selection.existing_library()?;
    SELECTION
        .set(selection)
        .map_err(|_| RError::LibraryNotFound("An R library has already been selected".into()))
}

impl Selection {
    fn existing_library(&self) -> RResult<PathBuf> {
        if !self.library.is_absolute() || !self.library.is_file() {
            return Err(RError::LibraryNotFound(
                "The selected R shared library is unavailable".into(),
            ));
        }
        Ok(self.library.clone())
    }
}

pub(crate) fn selected_library() -> Option<RResult<PathBuf>> {
    SELECTION.get().map(Selection::existing_library)
}

#[cfg(windows)]
pub(crate) fn dependency_prefix() -> Option<&'static Path> {
    SELECTION
        .get()
        .and_then(|selection| selection.dependency_prefix.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_selected_library_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("libR");
        std::fs::write(&library, "fixture").unwrap();
        let selection = Selection {
            library: library.clone(),
            dependency_prefix: None,
        };
        assert_eq!(selection.existing_library().unwrap(), library);
        std::fs::remove_file(library).unwrap();
        assert!(selection.existing_library().is_err());
    }
}
