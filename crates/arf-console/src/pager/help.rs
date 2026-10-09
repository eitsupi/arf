//! Interactive fuzzy search for R documentation.
//!
//! This module provides a terminal-based fuzzy search interface for R help topics
//! loaded from installed packages' `Meta/Rd.rds`, `Meta/vignette.rds`, and
//! `Meta/demo.rds` independently.
//!
//! # Acknowledgment
//!
//! This implementation is inspired by the **felp** package by Atsushi Yasumoto (atusy):
//! - Repository: <https://github.com/atusy/felp>
//! - CRAN: <https://cran.r-project.org/package=felp>
//!
//! The concept of fuzzy help search was learned from felp's `fuzzyhelp()` function;
//! help indexes are read directly from installed packages here.

mod browser;
mod pages;
mod search;

pub use browser::run_help_browser;
pub(crate) use pages::display_prepared_help_request;
pub(super) use pages::{HelpPageSelectorState, help_page_title};

use arf_harp::HarpResult;

const MAX_FILTERED_RESULTS: usize = 500;
const MIN_SIZE: super::MinimumSize = super::MinimumSize { cols: 30, rows: 8 };

fn help_library_paths_after_refresh(
    refresh: HarpResult<Vec<String>>,
    cached: impl FnOnce() -> Vec<String>,
) -> HarpResult<Vec<String>> {
    match refresh {
        Ok(paths) => Ok(paths),
        Err(error) => {
            let paths = cached();
            if paths.is_empty() {
                return Err(error);
            }
            log::warn!(
                "Could not refresh R library paths for help; using the previous snapshot: {error}"
            );
            Ok(paths)
        }
    }
}
