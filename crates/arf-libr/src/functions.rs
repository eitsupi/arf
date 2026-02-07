//! R function bindings loaded at runtime.

use crate::error::{RError, RResult};
use crate::types::*;
use libloading::{Library, Symbol};
use once_cell::sync::OnceCell;
use std::os::raw::{c_char, c_int};
use std::path::Path;

/// Global R library instance.
static R_LIBRARY: OnceCell<RLibrary> = OnceCell::new();

/// Preloaded supporting DLLs on Windows.
/// These are kept loaded so R packages can find them via the "Loaded-module list".
#[cfg(windows)]
static PRELOADED_DLLS: OnceCell<Vec<Library>> = OnceCell::new();

/// Container for the loaded R library and function pointers.
pub struct RLibrary {
    _library: Library,
    // Core functions
    pub rf_initialize_r: unsafe extern "C" fn(c_int, *const *const c_char) -> c_int,
    pub setup_rmainloop: unsafe extern "C" fn(),
    pub run_rmainloop: unsafe extern "C" fn(),
    pub rf_endembeddedr: unsafe extern "C" fn(c_int),

    // Parsing and evaluation
    pub r_parsevector: unsafe extern "C" fn(SEXP, c_int, *mut ParseStatus, SEXP) -> SEXP,
    pub rf_protect: unsafe extern "C" fn(SEXP) -> SEXP,
    pub rf_unprotect: unsafe extern "C" fn(c_int),
    pub r_tryeval: unsafe extern "C" fn(SEXP, SEXP, *mut c_int) -> SEXP,

    // String functions
    pub rf_mkchar: unsafe extern "C" fn(*const c_char) -> SEXP,
    pub rf_mkstring: unsafe extern "C" fn(*const c_char) -> SEXP,
    pub rf_scalarstringmaybe: unsafe extern "C" fn(SEXP) -> SEXP,
    pub r_charsxp: unsafe extern "C" fn(SEXP) -> *const c_char,

    // Vector functions
    pub rf_allocvector: unsafe extern "C" fn(c_int, isize) -> SEXP,
    pub rf_length: unsafe extern "C" fn(SEXP) -> c_int,
    pub rf_xlength: unsafe extern "C" fn(SEXP) -> isize,
    pub set_string_elt: unsafe extern "C" fn(SEXP, isize, SEXP),
    pub string_elt: unsafe extern "C" fn(SEXP, isize) -> SEXP,
    pub vector_elt: unsafe extern "C" fn(SEXP, isize) -> SEXP,

    // Type checking
    pub rf_typeof: unsafe extern "C" fn(SEXP) -> c_int,
    pub rf_isstring: unsafe extern "C" fn(SEXP) -> Rboolean,

    // Output
    pub rf_printvalue: unsafe extern "C" fn(SEXP),

    // Symbol installation
    pub rf_install: unsafe extern "C" fn(*const c_char) -> SEXP,

    // List construction
    pub rf_lcons: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
    pub rf_cons: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,

    // Logical vector access
    pub logical: unsafe extern "C" fn(SEXP) -> *mut c_int,

    // Integer vector access
    pub integer: unsafe extern "C" fn(SEXP) -> *mut c_int,

    // Top-level execution (for safe error handling)
    pub r_toplevelexec: unsafe extern "C" fn(
        Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
        *mut std::ffi::c_void,
    ) -> Rboolean,

    // Eval (without error handling - use inside R_ToplevelExec)
    pub rf_eval: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,

    // Global symbols
    pub r_nilvalue: *mut SEXP,
    pub r_globalenv: *mut SEXP,
    pub r_baseenv: *mut SEXP,
    pub r_unboundvalue: *mut SEXP,

    // Environment and variable manipulation
    // Rf_findVar searches through enclosing environments
    pub rf_findvar: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
    pub rf_definevar: unsafe extern "C" fn(SEXP, SEXP, SEXP),
    pub rf_scalarlogical: unsafe extern "C" fn(c_int) -> SEXP,

    // Stack limit (for embedded R)
    pub r_cstacklimit: *mut usize,

    // Console callbacks (Unix only - Windows uses Rstart params)
    #[cfg(unix)]
    pub ptr_r_readconsole: *mut ReadConsoleFunc,
    #[cfg(unix)]
    pub ptr_r_writeconsoleex: *mut WriteConsoleExFunc,
    #[cfg(unix)]
    pub ptr_r_writeconsole: *mut Option<unsafe extern "C" fn(*const c_char, c_int)>,

    // Console file pointers (must be NULL for callbacks to work)
    #[cfg(unix)]
    pub r_consolefile: *mut *mut std::ffi::c_void,
    #[cfg(unix)]
    pub r_outputfile: *mut *mut std::ffi::c_void,

    // Suicide callback (fatal error handler) - Unix only
    #[cfg(unix)]
    pub ptr_r_suicide: *mut Option<unsafe extern "C-unwind" fn(*const c_char)>,

    // Windows-specific initialization functions
    #[cfg(windows)]
    pub r_defparamsex: unsafe extern "C" fn(*mut crate::types::Rstart, c_int),
    #[cfg(windows)]
    pub r_setparams: unsafe extern "C" fn(*mut crate::types::Rstart),
    #[cfg(windows)]
    pub cmdlineoptions: unsafe extern "C" fn(c_int, *mut *mut c_char),
    #[cfg(windows)]
    pub r_common_command_line:
        unsafe extern "C" fn(*mut c_int, *mut *mut c_char, *mut crate::types::Rstart),

    // Windows readconsolecfg (from R.dll, not Rgraphapp.dll)
    #[cfg(windows)]
    pub readconsolecfg: unsafe extern "C" fn(),

    // Windows getRUser (returns R's `~` home directory)
    // Search order: R_USER → HOME → SHGetKnownFolderPath(Documents) → HOMEDRIVE+HOMEPATH → cwd
    #[cfg(windows)]
    pub get_r_user: unsafe extern "C" fn() -> *const c_char,

    // Windows Rgraphapp.dll functions (loaded separately, optional)
    #[cfg(windows)]
    pub ga_initapp: Option<unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int>,

    // R state variables
    pub r_interactive: *mut c_int,
    pub r_signalhandlers: *mut c_int,
    pub r_running_as_main_program: *mut c_int,

    // Event processing functions
    pub r_processevents: unsafe extern "C" fn(),

    // Windows-specific event processing (from Rgraphapp.dll)
    #[cfg(windows)]
    pub ga_peekevent: Option<unsafe extern "C" fn() -> c_int>,

    // Unix-specific event processing
    #[cfg(unix)]
    pub r_checkactivity: unsafe extern "C" fn(c_int, c_int) -> *mut std::ffi::c_void,
    #[cfg(unix)]
    pub r_runhandlers: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void),
    #[cfg(unix)]
    pub r_inputhandlers: *mut *mut std::ffi::c_void,

    // R_PolledEvents callback pointer (called by R periodically)
    #[cfg(unix)]
    pub ptr_r_polledevents: *mut Option<unsafe extern "C" fn()>,
}

// Safety: RLibrary contains only function pointers and raw pointers that are
// used in a thread-safe manner (R is single-threaded anyway).
unsafe impl Send for RLibrary {}
unsafe impl Sync for RLibrary {}

impl RLibrary {
    /// Load the R library from the given path.
    ///
    /// On Unix, the library is loaded with RTLD_GLOBAL so that R packages
    /// can find libR.so symbols when loading their own shared libraries.
    ///
    /// On Windows, we preemptively load supporting R DLLs (Rgraphapp, Rlapack,
    /// Riconv, Rblas) before loading R.dll. This is necessary because R packages
    /// (including base packages like 'stats') link to these DLLs, and Windows
    /// searches the "Loaded-module list" when resolving DLL dependencies.
    pub fn load(library_path: &Path) -> RResult<Self> {
        unsafe {
            #[cfg(unix)]
            let library = {
                use libloading::os::unix::Library as UnixLibrary;
                // RTLD_NOW = 0x2, RTLD_GLOBAL = 0x100
                const RTLD_NOW: libc::c_int = 0x2;
                const RTLD_GLOBAL: libc::c_int = 0x100;
                let unix_lib = UnixLibrary::open(Some(library_path), RTLD_NOW | RTLD_GLOBAL)
                    .map_err(|e| RError::LibraryNotFound(e.to_string()))?;
                Library::from(unix_lib)
            };

            #[cfg(windows)]
            let library = {
                use libloading::os::windows::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR;
                use libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32;
                use libloading::os::windows::Library as WinLibrary;

                // Preload supporting DLLs before loading R.dll.
                // These must be loaded first so they're in the "Loaded-module list"
                // when R packages (like stats) try to resolve their dependencies.
                // See: https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order
                let dll_dir = library_path.parent().ok_or_else(|| {
                    RError::LibraryNotFound("Cannot determine R DLL directory".to_string())
                })?;

                let _ = PRELOADED_DLLS.get_or_init(|| {
                    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
                    let support_dlls = ["Rblas.dll", "Riconv.dll", "Rlapack.dll", "Rgraphapp.dll"];
                    let mut loaded = Vec::new();

                    for dll_name in &support_dlls {
                        let dll_path = dll_dir.join(dll_name);
                        if dll_path.exists() {
                            match WinLibrary::load_with_flags(&dll_path, flags) {
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
                let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
                let win_lib = WinLibrary::load_with_flags(library_path, flags)
                    .map_err(|e| RError::LibraryNotFound(e.to_string()))?;
                Library::from(win_lib)
            };

            macro_rules! load_symbol {
                ($name:ident, $sym:expr) => {
                    let $name: Symbol<_> = library.get($sym).map_err(|_| {
                        RError::FunctionNotFound(String::from_utf8_lossy($sym).to_string())
                    })?;
                    let $name = *$name;
                };
            }

            // Macro for loading global symbol pointers (platform-specific)
            // On Unix: Symbol::into_raw() returns os::unix::Symbol, then .into_raw() returns *mut c_void
            // On Windows: Symbol::into_raw() returns os::windows::Symbol<T>, then .into_raw() returns Option<FARPROC>
            #[cfg(unix)]
            macro_rules! load_ptr {
                ($name:ident, $sym:expr, $ty:ty) => {
                    let $name: Symbol<$ty> = library.get($sym).map_err(|_| {
                        RError::FunctionNotFound(String::from_utf8_lossy($sym).to_string())
                    })?;
                    let $name = $name.into_raw().into_raw() as *mut $ty;
                };
            }

            #[cfg(windows)]
            macro_rules! load_ptr {
                ($name:ident, $sym:expr, $ty:ty) => {
                    let $name: Symbol<$ty> = library.get($sym).map_err(|_| {
                        RError::FunctionNotFound(String::from_utf8_lossy($sym).to_string())
                    })?;
                    // On Windows, into_raw() returns os::windows::Symbol, then into_raw() returns Option<FARPROC>
                    // FARPROC is the address of the symbol, unwrap and cast to pointer
                    let $name = $name
                        .into_raw()
                        .into_raw()
                        .map(|f| f as usize as *mut $ty)
                        .unwrap_or(std::ptr::null_mut());
                };
            }

            // Load core functions
            load_symbol!(rf_initialize_r, b"Rf_initialize_R\0");
            load_symbol!(setup_rmainloop, b"setup_Rmainloop\0");
            load_symbol!(run_rmainloop, b"run_Rmainloop\0");
            load_symbol!(rf_endembeddedr, b"Rf_endEmbeddedR\0");

            // Load parsing and evaluation functions
            load_symbol!(r_parsevector, b"R_ParseVector\0");
            load_symbol!(rf_protect, b"Rf_protect\0");
            load_symbol!(rf_unprotect, b"Rf_unprotect\0");
            load_symbol!(r_tryeval, b"R_tryEval\0");

            // Load string functions
            load_symbol!(rf_mkchar, b"Rf_mkChar\0");
            load_symbol!(rf_mkstring, b"Rf_mkString\0");
            load_symbol!(rf_scalarstringmaybe, b"Rf_ScalarString\0");
            load_symbol!(r_charsxp, b"R_CHAR\0");

            // Load vector functions
            load_symbol!(rf_allocvector, b"Rf_allocVector\0");
            load_symbol!(rf_length, b"Rf_length\0");
            load_symbol!(rf_xlength, b"Rf_xlength\0");
            load_symbol!(set_string_elt, b"SET_STRING_ELT\0");
            load_symbol!(string_elt, b"STRING_ELT\0");
            load_symbol!(vector_elt, b"VECTOR_ELT\0");

            // Load type checking functions
            load_symbol!(rf_typeof, b"TYPEOF\0");
            load_symbol!(rf_isstring, b"Rf_isString\0");

            // Load output functions
            load_symbol!(rf_printvalue, b"Rf_PrintValue\0");

            // Load symbol installation
            load_symbol!(rf_install, b"Rf_install\0");

            // Load list construction
            load_symbol!(rf_lcons, b"Rf_lcons\0");
            load_symbol!(rf_cons, b"Rf_cons\0");

            // Load logical access
            load_symbol!(logical, b"LOGICAL\0");

            // Load integer access
            load_symbol!(integer, b"INTEGER\0");

            // Load top-level execution
            load_symbol!(r_toplevelexec, b"R_ToplevelExec\0");
            load_symbol!(rf_eval, b"Rf_eval\0");

            // Load global symbols
            load_ptr!(r_nilvalue, b"R_NilValue\0", SEXP);
            load_ptr!(r_globalenv, b"R_GlobalEnv\0", SEXP);
            load_ptr!(r_baseenv, b"R_BaseEnv\0", SEXP);
            load_ptr!(r_unboundvalue, b"R_UnboundValue\0", SEXP);

            // Load environment and variable manipulation functions
            // Rf_findVar takes (symbol, env) and searches through enclosing environments
            load_symbol!(rf_findvar, b"Rf_findVar\0");
            load_symbol!(rf_definevar, b"Rf_defineVar\0");
            load_symbol!(rf_scalarlogical, b"Rf_ScalarLogical\0");

            // Load stack limit pointer
            load_ptr!(r_cstacklimit, b"R_CStackLimit\0", usize);

            // Load console callbacks (Unix only - Windows uses Rstart params)
            #[cfg(unix)]
            load_ptr!(ptr_r_readconsole, b"ptr_R_ReadConsole\0", ReadConsoleFunc);
            #[cfg(unix)]
            load_ptr!(
                ptr_r_writeconsoleex,
                b"ptr_R_WriteConsoleEx\0",
                WriteConsoleExFunc
            );
            #[cfg(unix)]
            load_ptr!(
                ptr_r_writeconsole,
                b"ptr_R_WriteConsole\0",
                Option<unsafe extern "C" fn(*const c_char, c_int)>
            );

            // Load console file pointers (Unix only)
            #[cfg(unix)]
            load_ptr!(r_consolefile, b"R_Consolefile\0", *mut std::ffi::c_void);
            #[cfg(unix)]
            load_ptr!(r_outputfile, b"R_Outputfile\0", *mut std::ffi::c_void);

            // Load suicide callback pointer (Unix only)
            #[cfg(unix)]
            load_ptr!(
                ptr_r_suicide,
                b"ptr_R_Suicide\0",
                Option<unsafe extern "C-unwind" fn(*const c_char)>
            );

            // Load Windows-specific initialization functions
            #[cfg(windows)]
            load_symbol!(r_defparamsex, b"R_DefParamsEx\0");
            #[cfg(windows)]
            load_symbol!(r_setparams, b"R_SetParams\0");
            #[cfg(windows)]
            load_symbol!(cmdlineoptions, b"cmdlineoptions\0");
            #[cfg(windows)]
            load_symbol!(r_common_command_line, b"R_common_command_line\0");
            // readconsolecfg is exported from R.dll, not Rgraphapp.dll
            #[cfg(windows)]
            load_symbol!(readconsolecfg, b"readconsolecfg\0");
            // getRUser returns R's `~` home directory as seen by R
            #[cfg(windows)]
            load_symbol!(get_r_user, b"getRUser\0");

            // Load Rgraphapp.dll functions (GA_initapp, GA_peekevent)
            #[cfg(windows)]
            let (ga_initapp, ga_peekevent) = {
                // Find Rgraphapp.dll in the same directory as R.dll
                let rgraphapp_path = library_path.parent().map(|p| p.join("Rgraphapp.dll"));

                log::info!(
                    "[WINDOWS] Looking for Rgraphapp.dll at: {:?}",
                    rgraphapp_path
                );

                if let Some(ref path) = rgraphapp_path {
                    if path.exists() {
                        log::info!("[WINDOWS] Rgraphapp.dll exists, loading...");
                        use libloading::os::windows::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR;
                        use libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32;
                        use libloading::os::windows::Library as WinLibrary;
                        let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;

                        match WinLibrary::load_with_flags(path, flags) {
                            Ok(graphapp_lib) => {
                                log::info!("[WINDOWS] Rgraphapp.dll loaded successfully");
                                let graphapp_lib = Library::from(graphapp_lib);

                                let ga_initapp: Option<
                                    unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int,
                                > = graphapp_lib
                                    .get::<unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int>(
                                        b"GA_initapp\0",
                                    )
                                    .ok()
                                    .map(|s| *s);
                                log::info!(
                                    "[WINDOWS] GA_initapp: {}",
                                    if ga_initapp.is_some() {
                                        "found"
                                    } else {
                                        "not found"
                                    }
                                );

                                let ga_peekevent: Option<unsafe extern "C" fn() -> c_int> =
                                    graphapp_lib
                                        .get::<unsafe extern "C" fn() -> c_int>(b"GA_peekevent\0")
                                        .ok()
                                        .map(|s| *s);
                                log::info!(
                                    "[WINDOWS] GA_peekevent: {}",
                                    if ga_peekevent.is_some() {
                                        "found"
                                    } else {
                                        "not found"
                                    }
                                );

                                // Leak the library so it stays loaded
                                std::mem::forget(graphapp_lib);

                                (ga_initapp, ga_peekevent)
                            }
                            Err(e) => {
                                log::warn!("[WINDOWS] Failed to load Rgraphapp.dll: {:?}", e);
                                (None, None)
                            }
                        }
                    } else {
                        log::warn!("[WINDOWS] Rgraphapp.dll not found at {:?}", path);
                        (None, None)
                    }
                } else {
                    log::warn!("[WINDOWS] Could not determine Rgraphapp.dll path");
                    (None, None)
                }
            };

            // Load R state variables (as raw pointers - these are global ints, not pointers)
            // We need to get the address of these symbols, not their values
            #[cfg(unix)]
            let r_interactive: *mut c_int = library
                .get::<c_int>(b"R_Interactive\0")
                .map(|s| s.into_raw().into_raw() as *mut c_int)
                .unwrap_or(std::ptr::null_mut());
            #[cfg(unix)]
            let r_signalhandlers: *mut c_int = library
                .get::<c_int>(b"R_SignalHandlers\0")
                .map(|s| s.into_raw().into_raw() as *mut c_int)
                .unwrap_or(std::ptr::null_mut());
            #[cfg(unix)]
            let r_running_as_main_program: *mut c_int = library
                .get::<c_int>(b"R_running_as_main_program\0")
                .map(|s| s.into_raw().into_raw() as *mut c_int)
                .unwrap_or(std::ptr::null_mut());

            #[cfg(windows)]
            let r_interactive: *mut c_int = library
                .get::<c_int>(b"R_Interactive\0")
                .ok()
                .and_then(|s| s.into_raw().into_raw().map(|f| f as usize as *mut c_int))
                .unwrap_or(std::ptr::null_mut());
            #[cfg(windows)]
            let r_signalhandlers: *mut c_int = library
                .get::<c_int>(b"R_SignalHandlers\0")
                .ok()
                .and_then(|s| s.into_raw().into_raw().map(|f| f as usize as *mut c_int))
                .unwrap_or(std::ptr::null_mut());
            #[cfg(windows)]
            let r_running_as_main_program: *mut c_int = library
                .get::<c_int>(b"R_running_as_main_program\0")
                .ok()
                .and_then(|s| s.into_raw().into_raw().map(|f| f as usize as *mut c_int))
                .unwrap_or(std::ptr::null_mut());

            // Load event processing function (cross-platform)
            load_symbol!(r_processevents, b"R_ProcessEvents\0");

            // Load Unix-specific event processing functions
            #[cfg(unix)]
            load_symbol!(r_checkactivity, b"R_checkActivity\0");
            #[cfg(unix)]
            load_symbol!(r_runhandlers, b"R_runHandlers\0");
            #[cfg(unix)]
            let r_inputhandlers: *mut *mut std::ffi::c_void = library
                .get::<*mut std::ffi::c_void>(b"R_InputHandlers\0")
                .map(|s| s.into_raw().into_raw() as *mut *mut std::ffi::c_void)
                .unwrap_or(std::ptr::null_mut());
            #[cfg(unix)]
            load_ptr!(
                ptr_r_polledevents,
                b"R_PolledEvents\0",
                Option<unsafe extern "C" fn()>
            );

            Ok(RLibrary {
                _library: library,
                rf_initialize_r,
                setup_rmainloop,
                run_rmainloop,
                rf_endembeddedr,
                r_parsevector,
                rf_protect,
                rf_unprotect,
                r_tryeval,
                rf_mkchar,
                rf_mkstring,
                rf_scalarstringmaybe,
                r_charsxp,
                rf_allocvector,
                rf_length,
                rf_xlength,
                set_string_elt,
                string_elt,
                vector_elt,
                rf_typeof,
                rf_isstring,
                rf_printvalue,
                rf_install,
                rf_lcons,
                rf_cons,
                logical,
                integer,
                r_toplevelexec,
                rf_eval,
                r_nilvalue,
                r_globalenv,
                r_baseenv,
                r_unboundvalue,
                rf_findvar,
                rf_definevar,
                rf_scalarlogical,
                r_cstacklimit,
                #[cfg(unix)]
                ptr_r_readconsole,
                #[cfg(unix)]
                ptr_r_writeconsoleex,
                #[cfg(unix)]
                ptr_r_writeconsole,
                #[cfg(unix)]
                r_consolefile,
                #[cfg(unix)]
                r_outputfile,
                #[cfg(unix)]
                ptr_r_suicide,
                #[cfg(windows)]
                r_defparamsex,
                #[cfg(windows)]
                r_setparams,
                #[cfg(windows)]
                cmdlineoptions,
                #[cfg(windows)]
                r_common_command_line,
                #[cfg(windows)]
                readconsolecfg,
                #[cfg(windows)]
                get_r_user,
                #[cfg(windows)]
                ga_initapp,
                r_interactive,
                r_signalhandlers,
                r_running_as_main_program,
                r_processevents,
                #[cfg(windows)]
                ga_peekevent,
                #[cfg(unix)]
                r_checkactivity,
                #[cfg(unix)]
                r_runhandlers,
                #[cfg(unix)]
                r_inputhandlers,
                #[cfg(unix)]
                ptr_r_polledevents,
            })
        }
    }
}

/// Initialize the global R library.
pub fn init_r_library(library_path: &Path) -> RResult<()> {
    R_LIBRARY
        .set(RLibrary::load(library_path)?)
        .map_err(|_| RError::EvalError("R library already initialized".to_string()))
}

/// Get a reference to the global R library.
pub fn r_library() -> RResult<&'static RLibrary> {
    R_LIBRARY.get().ok_or(RError::NotInitialized)
}

/// Get R_NilValue.
pub fn r_nil_value() -> RResult<SEXP> {
    let lib = r_library()?;
    unsafe { Ok(*lib.r_nilvalue) }
}

/// Get R_GlobalEnv.
pub fn r_global_env() -> RResult<SEXP> {
    let lib = r_library()?;
    unsafe { Ok(*lib.r_globalenv) }
}
