//! Temporary DLL search experiments, enabled only by the spike feature.

use std::path::Path;

use libloading::os::windows::{
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library,
};

pub(crate) fn configure(_library_path: &Path) -> crate::RResult<()> {
    #[cfg(feature = "pixi-embedding-spike")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{
            AddDllDirectory, LOAD_LIBRARY_SEARCH_USER_DIRS, SetDefaultDllDirectories,
        };

        let mode = std::env::var("ARF_SPIKE_DLL_SEARCH").unwrap_or_else(|_| "restricted".into());
        log::info!("[SPIKE] DLL search: {mode}");
        match mode.as_str() {
            "restricted" | "standard" => return Ok(()),
            "prefix" => {}
            _ => {
                return Err(crate::RError::LibraryNotFound(
                    "Unknown spike DLL policy".into(),
                ));
            }
        }
        let prefix = std::fs::canonicalize(
            std::env::var_os("CONDA_PREFIX")
                .ok_or_else(|| crate::RError::LibraryNotFound("Missing spike prefix".into()))?,
        )
        .map_err(|error| crate::RError::LibraryNotFound(error.to_string()))?;
        let library_dir = _library_path.parent().ok_or_else(|| {
            crate::RError::LibraryNotFound("Missing spike R DLL directory".into())
        })?;
        let paths = std::env::var_os("PATH").unwrap_or_default();
        let mut directories = Vec::new();
        for directory in
            std::iter::once(library_dir.to_path_buf()).chain(std::env::split_paths(&paths))
        {
            let Ok(directory) = std::fs::canonicalize(directory) else {
                continue;
            };
            if directory.is_dir()
                && directory.starts_with(&prefix)
                && !directories.contains(&directory)
            {
                directories.push(directory);
            }
        }
        if !std::fs::canonicalize(_library_path)
            .map_err(|error| crate::RError::LibraryNotFound(error.to_string()))?
            .starts_with(&prefix)
        {
            return Err(crate::RError::LibraryNotFound(
                "Spike R DLL is outside prefix".into(),
            ));
        }
        for directory in directories {
            let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: The path is absolute, exists, and is NUL terminated.
            // Keep the registration for the process lifetime so R's own package
            // loads use the same directories. No search order between added
            // directories is assumed.
            if unsafe { AddDllDirectory(wide.as_ptr()) }.is_null() {
                return Err(crate::RError::LibraryNotFound(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            log::info!("[SPIKE] DLL directory: {}", directory.display());
        }
        // SAFETY: Called during single-threaded R initialization. Excludes CWD,
        // PATH, and the executable directory from subsequent ordinary DLL loads.
        if unsafe {
            SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS)
        } == 0
        {
            return Err(crate::RError::LibraryNotFound(
                std::io::Error::last_os_error().to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) unsafe fn open(path: &Path) -> Result<Library, libloading::Error> {
    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
    #[cfg(feature = "pixi-embedding-spike")]
    let flags = match std::env::var("ARF_SPIKE_DLL_SEARCH").as_deref() {
        Ok("standard") => return unsafe { Library::new(path) },
        Ok("prefix") => {
            flags | windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_USER_DIRS
        }
        _ => flags,
    };
    // SAFETY: The caller ensures loading R and its dependencies is safe.
    unsafe { Library::load_with_flags(path, flags) }
}
