//! Platform-specific opening of the R shared library.

use crate::error::{RError, RResult};
use libloading::Library;
#[cfg(windows)]
use once_cell::sync::OnceCell;
use std::path::Path;

/// Preloaded supporting DLLs on Windows.
/// These are kept loaded so R packages can find them via the "Loaded-module list".
#[cfg(windows)]
static PRELOADED_DLLS: OnceCell<Vec<Library>> = OnceCell::new();

/// Open the R shared library with the loader flags R packages expect.
///
/// The library is opened with RTLD_GLOBAL so that R packages can find libR.so
/// symbols when loading their own shared libraries.
#[cfg(unix)]
pub(super) fn open_library(library_path: &Path) -> RResult<Library> {
    unsafe {
        use libloading::os::unix::Library as UnixLibrary;
        // RTLD_LAZY = 0x1, RTLD_GLOBAL = 0x100
        const RTLD_LAZY: libc::c_int = 0x1;
        const RTLD_GLOBAL: libc::c_int = 0x100;
        let unix_lib = UnixLibrary::open(Some(library_path), RTLD_LAZY | RTLD_GLOBAL)
            .map_err(|e| RError::LibraryNotFound(e.to_string()))?;
        Ok(Library::from(unix_lib))
    }
}

/// Open R.dll after preloading its supporting DLLs.
///
/// The supporting DLLs (Rgraphapp, Rlapack, Riconv, Rblas) must be loaded
/// first so that R packages linking to them can resolve their dependencies
/// through the "Loaded-module list".
#[cfg(windows)]
pub(super) fn open_library(library_path: &Path) -> RResult<Library> {
    unsafe {
        // Preload supporting DLLs before loading R.dll.
        // These must be loaded first so they're in the "Loaded-module list"
        // when R packages (like stats) try to resolve their dependencies.
        // See: https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order
        crate::windows_loader::configure(library_path)?;
        let dll_dir = library_path.parent().ok_or_else(|| {
            RError::LibraryNotFound("Cannot determine R DLL directory".to_string())
        })?;

        let _ = PRELOADED_DLLS.get_or_init(|| {
            let support_dlls = ["Rblas.dll", "Riconv.dll", "Rlapack.dll", "Rgraphapp.dll"];
            let mut loaded = Vec::new();

            for dll_name in &support_dlls {
                let dll_path = dll_dir.join(dll_name);
                if dll_path.exists() {
                    match crate::windows_loader::open(&dll_path) {
                        Ok(lib) => {
                            log::info!("[WINDOWS] Preloaded {}", dll_name);
                            loaded.push(Library::from(lib));
                        }
                        Err(e) => {
                            log::warn!("[WINDOWS] Failed to preload {}: {:?}", dll_name, e);
                        }
                    }
                } else {
                    log::debug!("[WINDOWS] {} not found at {:?}", dll_name, dll_path);
                }
            }

            loaded
        });

        // Now load R.dll with the same flags
        let win_lib = crate::windows_loader::open(library_path)
            .map_err(|e| RError::LibraryNotFound(e.to_string()))?;
        Ok(Library::from(win_lib))
    }
}
