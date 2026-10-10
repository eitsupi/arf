//! DLL policy for ordinary R and a provider's selected dependency prefix.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use libloading::os::windows::{
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library,
};
use windows_sys::Win32::System::LibraryLoader::{
    AddDllDirectory, LOAD_LIBRARY_SEARCH_USER_DIRS, SetDefaultDllDirectories,
};

pub(crate) fn configure(library: &Path) -> crate::RResult<()> {
    let Some(prefix) = crate::library_selection::dependency_prefix() else {
        return Ok(());
    };
    let prefix = prefix.canonicalize().map_err(loader_error)?;
    let library = library.canonicalize().map_err(loader_error)?;
    if !library.starts_with(&prefix) {
        return Err(crate::RError::LibraryNotFound(
            "Selected R DLL is outside its dependency prefix".into(),
        ));
    }
    let library_dir = library
        .parent()
        .ok_or_else(|| crate::RError::LibraryNotFound("Selected R DLL has no directory".into()))?;
    let paths = std::env::var_os("PATH").unwrap_or_default();
    let mut directories = Vec::new();
    for directory in std::iter::once(library_dir.to_path_buf()).chain(std::env::split_paths(&paths))
    {
        let Ok(directory) = directory.canonicalize() else {
            continue;
        };
        if directory.is_dir() && directory.starts_with(&prefix) && !directories.contains(&directory)
        {
            directories.push(directory);
        }
    }
    for directory in directories {
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: Absolute, existing, NUL-terminated directory. Registrations
        // live for this process so R's compiled package loads use the policy too.
        // AddDllDirectory does not promise an order among these directories.
        if unsafe { AddDllDirectory(wide.as_ptr()) }.is_null() {
            return Err(loader_error(std::io::Error::last_os_error()));
        }
    }
    // SAFETY: Called during R initialization, before package loads. Ordinary R
    // never enters this branch. Exclude CWD, arbitrary PATH and application dirs.
    if unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS)
    } == 0
    {
        return Err(loader_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn loader_error(error: std::io::Error) -> crate::RError {
    crate::RError::LibraryNotFound(format!("Could not configure R DLL search: {error}"))
}

pub(crate) unsafe fn open(path: &Path) -> Result<Library, libloading::Error> {
    let mut flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
    if crate::library_selection::dependency_prefix().is_some() {
        flags |= LOAD_LIBRARY_SEARCH_USER_DIRS;
    }
    // SAFETY: The caller guarantees initialization and loading R is safe.
    unsafe { Library::load_with_flags(path, flags) }
}
