//! Reading R values without letting an R error unwind through Rust frames.
//!
//! Most R API functions that allocate, convert encodings or read through an
//! ALTREP object can raise an R error. Each such call here runs inside
//! [`top_level_exec`], whose contract (no value with a destructor alive across
//! a call into R) the closures below follow: they only handle `Copy` data, and
//! the one buffer they fill is owned by the caller and touched only after the
//! last fallible R call.

use super::error::JsonErrorKind;
use super::limits::MAX_STRING_BYTES;
use crate::ffi::top_level_exec;
use arf_libr::{CE_BYTES, RLibrary, SEXP, SexpType, r_nil_value};

/// The protected call did not complete: R signalled an error or the
/// evaluation was interrupted.
#[derive(Debug, Clone, Copy)]
pub(super) struct RJumped;

/// [`top_level_exec`] with the failure reduced to [`RJumped`].
///
/// # Safety
/// As for [`top_level_exec`].
unsafe fn protected<F, T>(f: F) -> Result<T, RJumped>
where
    F: FnOnce() -> T + Copy,
    T: Copy,
{
    unsafe { top_level_exec(f) }.map_err(|_| RJumped)
}

/// Why a CHARSXP could not be turned into a Rust string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StringError {
    Bytes,
    Invalid,
    /// The string does not fit in the remaining string budget.
    TooLong,
    Jumped,
}

impl From<StringError> for JsonErrorKind {
    fn from(error: StringError) -> Self {
        match error {
            StringError::Bytes => JsonErrorKind::BytesEncoding,
            StringError::Invalid => JsonErrorKind::InvalidEncoding,
            StringError::TooLong => JsonErrorKind::StringBytesExceeded {
                limit: MAX_STRING_BYTES,
            },
            StringError::Jumped => JsonErrorKind::RError,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Translation {
    Done,
    Bytes,
    TooLong,
}

/// Copy a CHARSXP into an owned UTF-8 `String`, translating from its declared
/// (or the native) encoding.
///
/// `max_bytes` is the most the caller can still accept. The size is checked
/// before translating (on the stored bytes) and again before copying (on the
/// translated bytes), so an oversized string is never translated into a second
/// buffer or copied only to be thrown away.
///
/// The translated buffer R hands back is temporary: it is copied inside the
/// protected call, immediately after `Rf_translateCharUTF8` returns, and R's
/// transient allocation mark is restored before leaving, so no pointer into
/// R-owned scratch memory survives the call. Translation of a string in a
/// non-UTF-8 native encoding is whatever R makes of it; see the module
/// documentation of [`super`].
///
/// # Safety
/// `charsxp` must be a valid CHARSXP that stays protected until this returns.
pub(super) unsafe fn charsxp_to_string(
    lib: &'static RLibrary,
    charsxp: SEXP,
    max_bytes: usize,
) -> Result<String, StringError> {
    let mut buffer: Vec<u8> = Vec::new();
    let out: *mut Vec<u8> = &mut buffer;
    // SAFETY: `out` stays valid for the call; it is dereferenced only after
    // the fallible R call has returned, so a jump never sees a live borrow.
    let translation = unsafe {
        protected(move || {
            if (lib.rf_getcharce)(charsxp) == CE_BYTES {
                return Translation::Bytes;
            }
            if (lib.rf_xlength)(charsxp).max(0) as usize > max_bytes {
                return Translation::TooLong;
            }
            let mark = (lib.vmaxget)();
            let chars = (lib.rf_translatecharutf8)(charsxp);
            let bytes = std::ffi::CStr::from_ptr(chars).to_bytes();
            if bytes.len() > max_bytes {
                (lib.vmaxset)(mark);
                return Translation::TooLong;
            }
            (*out).extend_from_slice(bytes);
            (lib.vmaxset)(mark);
            Translation::Done
        })
    }
    .map_err(|_| StringError::Jumped)?;
    match translation {
        Translation::Bytes => Err(StringError::Bytes),
        Translation::TooLong => Err(StringError::TooLong),
        Translation::Done => String::from_utf8(buffer).map_err(|_| StringError::Invalid),
    }
}

/// `VECTOR_ELT`, which may run an ALTREP element method that raises an error.
///
/// # Safety
/// `list` must be a valid, reachable VECSXP and `index` in range.
pub(super) unsafe fn list_elt(
    lib: &'static RLibrary,
    list: SEXP,
    index: usize,
) -> Result<SEXP, RJumped> {
    unsafe { protected(move || (lib.vector_elt)(list, index as isize)) }
}

/// `STRING_ELT`, which may run an ALTREP element method that raises an error.
///
/// # Safety
/// `strings` must be a valid, reachable STRSXP and `index` in range.
pub(super) unsafe fn string_elt(
    lib: &'static RLibrary,
    strings: SEXP,
    index: usize,
) -> Result<SEXP, RJumped> {
    unsafe { protected(move || (lib.string_elt)(strings, index as isize)) }
}

/// What a single R value looks like, as far as the JSON conversion cares.
///
/// Everything here is plain data so that it can leave a protected call.
#[derive(Debug, Clone, Copy)]
pub(super) enum Probe {
    Null,
    Logical(i32),
    Integer(i32),
    Real(f64),
    /// A character vector of length 1, holding its element (a CHARSXP).
    String(SEXP),
    /// A list, with its `names` attribute if it has one.
    List {
        len: usize,
        names: Option<SEXP>,
    },
    Reject(Reject),
}

/// Why a value has no JSON counterpart.
#[derive(Debug, Clone, Copy)]
pub(super) enum Reject {
    Type(i32),
    Length(usize),
    Class,
    MalformedNames,
}

/// Classify `x`. Reads the class and names attributes, the length and the
/// single element of an atomic vector in one protected call.
///
/// # Safety
/// `x` must be a valid SEXP that stays protected until this returns.
pub(super) unsafe fn probe(lib: &'static RLibrary, x: SEXP) -> Result<Probe, RJumped> {
    let nil = r_nil_value().map_err(|_| RJumped)?;
    unsafe {
        protected(move || {
            let sexp_type = (lib.rf_typeof)(x);
            if sexp_type == SexpType::NilSxp as i32 {
                return Probe::Null;
            }
            let supported = [
                SexpType::LglSxp,
                SexpType::IntSxp,
                SexpType::RealSxp,
                SexpType::StrSxp,
                SexpType::VecSxp,
            ]
            .iter()
            .any(|t| *t as i32 == sexp_type);
            if !supported {
                return Probe::Reject(Reject::Type(sexp_type));
            }
            if (lib.rf_getattrib)(x, *lib.r_classsymbol) != nil {
                return Probe::Reject(Reject::Class);
            }

            let len = (lib.rf_xlength)(x).max(0) as usize;
            if sexp_type == SexpType::VecSxp as i32 {
                let names = (lib.rf_getattrib)(x, *lib.r_namessymbol);
                if names == nil {
                    return Probe::List { len, names: None };
                }
                if (lib.rf_typeof)(names) != SexpType::StrSxp as i32
                    || (lib.rf_xlength)(names).max(0) as usize != len
                {
                    return Probe::Reject(Reject::MalformedNames);
                }
                return Probe::List {
                    len,
                    names: Some(names),
                };
            }
            if len != 1 {
                return Probe::Reject(Reject::Length(len));
            }
            if sexp_type == SexpType::LglSxp as i32 {
                Probe::Logical(*(lib.logical)(x))
            } else if sexp_type == SexpType::IntSxp as i32 {
                Probe::Integer(*(lib.integer)(x))
            } else if sexp_type == SexpType::RealSxp as i32 {
                Probe::Real(*(lib.real)(x))
            } else {
                Probe::String((lib.string_elt)(x, 0))
            }
        })
    }
}

/// A human-readable name for an R `TYPEOF` code.
pub(super) fn type_name(code: i32) -> String {
    let name = match code {
        0 => "NULL",
        1 => "symbol",
        2 => "pairlist",
        3 => "closure",
        4 => "environment",
        5 => "promise",
        6 => "language",
        7 => "special",
        8 => "builtin",
        9 => "char",
        10 => "logical",
        13 => "integer",
        14 => "double",
        15 => "complex",
        16 => "character",
        17 => "...",
        18 => "any",
        19 => "list",
        20 => "expression",
        21 => "bytecode",
        22 => "externalptr",
        23 => "weakref",
        24 => "raw",
        25 => "S4",
        other => return format!("type {other}"),
    };
    name.to_string()
}
