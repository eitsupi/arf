//! R type definitions.
//!
//! These types mirror R's internal C types.

use std::os::raw::{c_char, c_int};

/// R's SEXPREC structure (opaque).
#[repr(C)]
pub struct SEXPREC {
    _private: [u8; 0],
}

/// SEXP is a pointer to SEXPREC.
pub type SEXP = *mut SEXPREC;

/// R's Rboolean type.
pub type Rboolean = c_int;

/// R TRUE value.
pub const R_TRUE: Rboolean = 1;

/// R FALSE value.
pub const R_FALSE: Rboolean = 0;

/// Parse status returned by R_ParseVector.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStatus {
    Null = 0,
    Ok = 1,
    Incomplete = 2,
    Error = 3,
    Eof = 4,
}

/// SEXP type tags.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SexpType {
    NilSxp = 0,
    SymSxp = 1,
    ListSxp = 2,
    ClosSxp = 3,
    EnvSxp = 4,
    PromSxp = 5,
    LangSxp = 6,
    SpecialSxp = 7,
    BuiltinSxp = 8,
    CharSxp = 9,
    LglSxp = 10,
    IntSxp = 13,
    RealSxp = 14,
    CplxSxp = 15,
    StrSxp = 16,
    DotSxp = 17,
    AnySxp = 18,
    VecSxp = 19,
    ExprSxp = 20,
    BcodeSxp = 21,
    ExtptrSxp = 22,
    WeakrefSxp = 23,
    RawSxp = 24,
    S4Sxp = 25,
}

/// Pointer to C function for console read callback.
pub type ReadConsoleFunc = Option<
    unsafe extern "C" fn(
        prompt: *const c_char,
        buf: *mut c_char,
        buflen: c_int,
        hist: c_int,
    ) -> c_int,
>;

/// Pointer to C function for console write callback.
pub type WriteConsoleExFunc =
    Option<unsafe extern "C" fn(buf: *const c_char, buflen: c_int, otype: c_int)>;

// Windows-specific types for R initialization
// On Windows, R uses a params struct instead of global function pointers

/// Windows UI mode.
#[cfg(windows)]
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UImode {
    RGui = 0,
    RTerm = 1,
    LinkDLL = 2,
}

/// Windows startup action type.
#[cfg(windows)]
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaType {
    NoRestore = 0,
    Restore = 1,
    Default = 2,
    NoSave = 3,
    Save = 4,
    SaveAsk = 5,
    Suicide = 6,
}

/// Windows Rstart structure for R initialization.
///
/// On Windows, R uses this struct to configure console callbacks
/// instead of global function pointers like ptr_R_ReadConsole.
#[cfg(windows)]
#[repr(C)]
pub struct Rstart {
    pub r_quiet: Rboolean,
    pub r_no_echo: Rboolean,
    pub r_interactive: Rboolean,
    pub r_verbose: Rboolean,
    pub load_site_file: Rboolean,
    pub load_init_file: Rboolean,
    pub debug_init_file: Rboolean,
    pub restore_action: SaType,
    pub save_action: SaType,
    pub vsize: usize,
    pub nsize: usize,
    pub max_vsize: usize,
    pub max_nsize: usize,
    pub ppsize: usize,
    // Bitfield: NoRenviron (16 bits) + RstartVersion (16 bits)
    pub bitfield: u32,
    /// R_HOME path
    pub rhome: *mut c_char,
    /// HOME path
    pub home: *mut c_char,
    pub read_console: ReadConsoleFunc,
    pub write_console: Option<unsafe extern "C" fn(*const c_char, c_int)>,
    /// ProcessEvents callback
    pub callback: Option<unsafe extern "C" fn()>,
    pub show_message: Option<unsafe extern "C" fn(*const c_char)>,
    pub yes_no_cancel: Option<unsafe extern "C" fn(*const c_char) -> c_int>,
    pub busy: Option<unsafe extern "C" fn(c_int)>,
    pub character_mode: UImode,
    pub write_console_ex: WriteConsoleExFunc,
    /// R 4.0.0+
    pub emit_embedded_utf8: Rboolean,
    /// R 4.2.0+ (RstartVersion 1)
    pub cleanup: Option<unsafe extern "C" fn(SaType, c_int, c_int)>,
    pub clearerr_console: Option<unsafe extern "C" fn()>,
    pub flush_console: Option<unsafe extern "C" fn()>,
    pub reset_console: Option<unsafe extern "C" fn()>,
    pub suicide: Option<unsafe extern "C" fn(*const c_char)>,
}
