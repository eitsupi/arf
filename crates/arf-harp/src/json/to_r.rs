//! Converting JSON values to R values.
//!
//! The work is split so that nothing owned by Rust is alive when R might
//! `longjmp`:
//!
//! 1. [`validate`] checks the whole document in pure Rust (limits, NUL
//!    characters, number ranges) and produces the detailed, located errors.
//! 2. The validated document is then built inside a single
//!    [`top_level_exec`] whose closure only touches borrowed data and raw
//!    `SEXP`s. If R raises an error (for example out of memory) the build is
//!    abandoned and the failure is reported as [`JsonErrorKind::RError`].
//! 3. Only after that has returned is the finished value wrapped in a
//!    protected [`RObject`].

use super::error::{JsonError, JsonErrorKind};
use super::limits::{Budget, MAX_EXACT_INTEGER, PathStack, check_depth};
use crate::ffi::top_level_exec;
use crate::object::RObject;
use arf_libr::{CE_UTF8, RLibrary, SEXP, SexpType, r_library};
use serde_json::{Number, Value};
use std::os::raw::c_int;

/// Convert a JSON value to a protected R value.
///
/// See the [module documentation](super) for the mapping and the checks made.
/// Must be called on the R thread.
pub fn json_to_r(value: &Value) -> Result<RObject, JsonError> {
    let lib =
        r_library().map_err(|e| JsonError::at_root(JsonErrorKind::RUnavailable(e.to_string())))?;

    let mut budget = Budget::default();
    let mut path = PathStack::default();
    validate(value, 0, &mut budget, &mut path)?;

    // SAFETY: the closure holds a reference and a `'static` reference only,
    // and `build` keeps its own intermediate objects protected.
    let built = unsafe { top_level_exec(move || build(lib, value)) }
        .map_err(|_| JsonError::at_root(JsonErrorKind::RError))?;
    // SAFETY: `built` is a fresh R value. No R allocation can happen between
    // the end of the build (which released its own protections) and this call,
    // so the value is still alive; this is the only protection held here.
    Ok(unsafe { RObject::new(built) })
}

/// Check everything about `value` that can be checked without R.
///
/// `depth` is the nesting level of the container holding `value`.
pub(super) fn validate(
    value: &Value,
    depth: usize,
    budget: &mut Budget,
    path: &mut PathStack,
) -> Result<(), JsonError> {
    budget.count_node().map_err(|kind| path.error(kind))?;
    match value {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::Number(number) => validate_number(number).map_err(|kind| path.error(kind)),
        Value::String(text) => validate_string(text, budget).map_err(|kind| path.error(kind)),
        Value::Array(items) => {
            check_depth(depth + 1).map_err(|kind| path.error(kind))?;
            for (index, item) in items.iter().enumerate() {
                path.push_index(index);
                validate(item, depth + 1, budget, path)?;
                path.pop();
            }
            Ok(())
        }
        Value::Object(members) => {
            check_depth(depth + 1).map_err(|kind| path.error(kind))?;
            for (key, member) in members {
                path.push_key(key);
                validate_string(key, budget).map_err(|kind| path.error(kind))?;
                validate(member, depth + 1, budget, path)?;
                path.pop();
            }
            Ok(())
        }
    }
}

fn validate_string(text: &str, budget: &mut Budget) -> Result<(), JsonErrorKind> {
    budget.count_string(text.len())?;
    if text.contains('\0') {
        return Err(JsonErrorKind::NulInString);
    }
    Ok(())
}

fn validate_number(number: &Number) -> Result<(), JsonErrorKind> {
    if let Some(integer) = number.as_i64() {
        if integer.unsigned_abs() > MAX_EXACT_INTEGER {
            return Err(JsonErrorKind::IntegerOutOfRange);
        }
    } else if let Some(integer) = number.as_u64() {
        if integer > MAX_EXACT_INTEGER {
            return Err(JsonErrorKind::IntegerOutOfRange);
        }
    } else if !number.as_f64().is_some_and(f64::is_finite) {
        return Err(JsonErrorKind::NonFiniteNumber);
    }
    Ok(())
}

/// Build the R value for a validated document.
///
/// This runs inside [`top_level_exec`], and any call into R may jump out of it.
/// Every frame on this path therefore holds only references, raw pointers and
/// integers: no `String`, `Vec`, guard or iterator adaptor with a destructor.
/// Intermediate R objects are protected with matching `Rf_protect` /
/// `Rf_unprotect` pairs, which R itself rewinds if it jumps. The returned
/// value is unprotected and must be used (stored or protected) before the
/// next allocation.
///
/// # Safety
/// `value` must have passed [`validate`], and the call must be made under
/// [`top_level_exec`] on the R thread.
unsafe fn build(lib: &'static RLibrary, value: &Value) -> SEXP {
    unsafe {
        match value {
            Value::Null => *lib.r_nilvalue,
            Value::Bool(flag) => (lib.rf_scalarlogical)(c_int::from(*flag)),
            Value::Number(number) => (lib.rf_scalarreal)(number.as_f64().unwrap_or(f64::NAN)),
            Value::String(text) => {
                let vector = (lib.rf_protect)((lib.rf_allocvector)(SexpType::StrSxp as c_int, 1));
                set_string(lib, vector, 0, text);
                (lib.rf_unprotect)(1);
                vector
            }
            Value::Array(items) => {
                let list = (lib.rf_protect)((lib.rf_allocvector)(
                    SexpType::VecSxp as c_int,
                    items.len() as isize,
                ));
                for (index, item) in items.iter().enumerate() {
                    let element = build(lib, item);
                    (lib.set_vector_elt)(list, index as isize, element);
                }
                (lib.rf_unprotect)(1);
                list
            }
            Value::Object(members) => {
                let list = (lib.rf_protect)((lib.rf_allocvector)(
                    SexpType::VecSxp as c_int,
                    members.len() as isize,
                ));
                let names = (lib.rf_protect)((lib.rf_allocvector)(
                    SexpType::StrSxp as c_int,
                    members.len() as isize,
                ));
                for (index, (key, member)) in members.iter().enumerate() {
                    set_string(lib, names, index, key);
                    let element = build(lib, member);
                    (lib.set_vector_elt)(list, index as isize, element);
                }
                // An empty object still gets a (zero-length) names attribute,
                // which is what tells it apart from an empty array.
                (lib.rf_setattrib)(list, *lib.r_namessymbol, names);
                (lib.rf_unprotect)(2);
                list
            }
        }
    }
}

/// Store `text` as a UTF-8 CHARSXP at `index` of the STRSXP `vector`.
///
/// # Safety
/// `vector` must be a protected STRSXP with room for `index`, and `text` must
/// not contain NUL.
unsafe fn set_string(lib: &'static RLibrary, vector: SEXP, index: usize, text: &str) {
    unsafe {
        let charsxp = (lib.rf_mkcharlence)(text.as_ptr().cast(), text.len() as c_int, CE_UTF8);
        (lib.set_string_elt)(vector, index as isize, charsxp);
    }
}
