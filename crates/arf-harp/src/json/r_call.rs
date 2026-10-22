//! Calling R's C API without letting an R error unwind through Rust frames.
//!
//! Most R API functions that allocate, convert encodings or read through an
//! ALTREP object can raise an R error, which is a `longjmp` that skips every
//! C and Rust frame between the failing call and the nearest R context.
//! Skipping a Rust frame that owns a `String`, `Vec`, map or guard leaks it
//! at best and is undefined behaviour at worst.
//!
//! This module confines each such call to a tiny callback run under
//! `R_ToplevelExec`, whose frames only hold plain data (`Copy` values), so
//! there is nothing to drop when R jumps over them. Everything with a
//! destructor stays on the Rust side of the boundary and sees the failure as
//! an ordinary `Err`.

use super::error::JsonErrorKind;
use arf_libr::{CE_BYTES, RLibrary, SEXP, SexpType, r_library, r_nil_value};
use std::ffi::c_void;

/// The protected call did not complete: R signalled an error or the
/// evaluation was interrupted.
#[derive(Debug, Clone, Copy)]
pub(super) struct RJumped;

struct Payload<F, T> {
    f: Option<F>,
    out: Option<T>,
}

unsafe extern "C" fn trampoline<F, T>(data: *mut c_void)
where
    F: FnOnce() -> T,
{
    // SAFETY: `data` points to the `Payload` owned by `r_protected`, which
    // outlives the `R_ToplevelExec` call.
    let payload = unsafe { &mut *(data as *mut Payload<F, T>) };
    if let Some(f) = payload.f.take() {
        payload.out = Some(f());
    }
}

/// Run `f` so that an R error inside it is reported as `Err(RJumped)`.
///
/// `f` and its result must be `Copy`: if R jumps out of `f`, the frames of
/// `f` and of the trampoline are discarded without running destructors, so
/// they must have none. By the same token, `f` must not create values with
/// destructors that are alive across a call into R. A closure may write into
/// memory the caller owns through a raw pointer, but only after the last call
/// into R that can fail.
///
/// # Safety
/// The caller must be on the R thread, with R initialized, and `f` may only
/// use R values that are protected or otherwise reachable.
pub(super) unsafe fn r_protected<F, T>(f: F) -> Result<T, RJumped>
where
    F: FnOnce() -> T + Copy,
    T: Copy,
{
    let lib = r_library().map_err(|_| RJumped)?;
    let mut payload = Payload::<F, T> {
        f: Some(f),
        out: None,
    };
    // SAFETY: the callback only touches `payload`, which lives across the call.
    let completed = unsafe {
        (lib.r_toplevelexec)(
            Some(trampoline::<F, T>),
            &mut payload as *mut Payload<F, T> as *mut c_void,
        )
    };
    match (completed, payload.out) {
        (0, _) | (_, None) => Err(RJumped),
        (_, Some(out)) => Ok(out),
    }
}

/// Why a CHARSXP could not be turned into a Rust string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StringError {
    Bytes,
    Invalid,
    Jumped,
}

impl From<StringError> for JsonErrorKind {
    fn from(error: StringError) -> Self {
        match error {
            StringError::Bytes => JsonErrorKind::BytesEncoding,
            StringError::Invalid => JsonErrorKind::InvalidEncoding,
            StringError::Jumped => JsonErrorKind::RError,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Translation {
    Done,
    Bytes,
}

/// Copy a CHARSXP into an owned UTF-8 `String`, translating from its declared
/// (or the native) encoding.
///
/// The translated buffer R hands back is temporary: it is copied inside the
/// protected call, immediately after `Rf_translateCharUTF8` returns, and R's
/// transient allocation mark is restored before leaving, so no pointer into
/// R-owned scratch memory survives the call.
///
/// # Safety
/// `charsxp` must be a valid, reachable CHARSXP.
pub(super) unsafe fn charsxp_to_string(
    lib: &'static RLibrary,
    charsxp: SEXP,
) -> Result<String, StringError> {
    let mut buffer: Vec<u8> = Vec::new();
    let out: *mut Vec<u8> = &mut buffer;
    // SAFETY: `out` stays valid for the call; it is dereferenced only after
    // the fallible R call has returned, so a jump never sees a live borrow.
    let translation = unsafe {
        r_protected(move || {
            if (lib.rf_getcharce)(charsxp) == CE_BYTES {
                return Translation::Bytes;
            }
            let mark = (lib.vmaxget)();
            let chars = (lib.rf_translatecharutf8)(charsxp);
            let bytes = std::ffi::CStr::from_ptr(chars).to_bytes();
            (*out).extend_from_slice(bytes);
            (lib.vmaxset)(mark);
            Translation::Done
        })
    }
    .map_err(|_| StringError::Jumped)?;
    match translation {
        Translation::Bytes => Err(StringError::Bytes),
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
    unsafe { r_protected(move || (lib.vector_elt)(list, index as isize)) }
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
    unsafe { r_protected(move || (lib.string_elt)(strings, index as isize)) }
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
    /// An attribute other than `names`, identified by its symbol.
    Attribute(SEXP),
    MalformedNames,
}

/// Classify `x`. Reads attributes, lengths and the single element of an
/// atomic vector in one protected call.
///
/// # Safety
/// `x` must be a valid, reachable SEXP.
pub(super) unsafe fn probe(lib: &'static RLibrary, x: SEXP) -> Result<Probe, RJumped> {
    let nil = r_nil_value().map_err(|_| RJumped)?;
    unsafe {
        r_protected(move || {
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

            let names_symbol = *lib.r_namessymbol;
            let class_symbol = *lib.r_classsymbol;
            let mut names = None;
            let mut other = None;
            let mut node = (lib.attrib)(x);
            while node != nil {
                let tag = (lib.tag)(node);
                if tag == names_symbol {
                    names = Some((lib.car)(node));
                } else if tag == class_symbol {
                    // A class wins over any other attribute, so every classed
                    // value is reported the same way.
                    return Probe::Reject(Reject::Class);
                } else if other.is_none() {
                    other = Some(tag);
                }
                node = (lib.cdr)(node);
            }
            if let Some(tag) = other {
                return Probe::Reject(Reject::Attribute(tag));
            }

            let len = (lib.rf_xlength)(x).max(0) as usize;
            if sexp_type == SexpType::VecSxp as i32 {
                if let Some(names) = names
                    && ((lib.rf_typeof)(names) != SexpType::StrSxp as i32
                        || (lib.rf_xlength)(names).max(0) as usize != len)
                {
                    return Probe::Reject(Reject::MalformedNames);
                }
                return Probe::List { len, names };
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
