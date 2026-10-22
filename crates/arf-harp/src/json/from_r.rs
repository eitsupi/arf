//! Converting R values to JSON values.

use super::error::{JsonError, JsonErrorKind};
use super::limits::{Budget, PathStack, check_depth};
use super::r_call::{Probe, Reject, charsxp_to_string, list_elt, probe, string_elt, type_name};
use crate::object::RObject;
use arf_libr::{RLibrary, SEXP, r_library};
use serde_json::{Map, Number, Value};

/// R's `NA_integer_` and `NA_logical_` are `INT_MIN`.
const NA_INTEGER: i32 = i32::MIN;

/// The low word R stores in the payload of `NA_real_`.
const NA_REAL_PAYLOAD: u64 = 1954;

/// Convert an R value to a JSON value.
///
/// See the [module documentation](super) for the mapping. Failures are
/// reported with the location of the offending element; they leave R's state
/// (protection stack, GC roots) untouched, so the next conversion works as if
/// nothing had happened.
///
/// Must be called on the R thread.
pub fn r_to_json(object: &RObject) -> Result<Value, JsonError> {
    let lib =
        r_library().map_err(|e| JsonError::at_root(JsonErrorKind::RUnavailable(e.to_string())))?;
    let mut converter = Converter {
        lib,
        budget: Budget::default(),
        path: PathStack::default(),
    };
    // SAFETY: `object` keeps its SEXP protected for the whole call, and every
    // value reached from it is protected in turn before it is used.
    unsafe { converter.convert(object.sexp(), 0) }
}

/// Convert an R value to compact JSON text.
///
/// See [`r_to_json`] for the mapping and the error behaviour.
pub fn r_to_json_text(object: &RObject) -> Result<String, JsonError> {
    r_to_json(object).map(|value| value.to_string())
}

struct Converter {
    lib: &'static RLibrary,
    budget: Budget,
    path: PathStack,
}

impl Converter {
    fn fail(&self, kind: JsonErrorKind) -> JsonError {
        self.path.error(kind)
    }

    /// Convert `x`, which sits `depth` containers below the root.
    ///
    /// # Safety
    /// `x` must be protected or reachable from a protected object.
    unsafe fn convert(&mut self, x: SEXP, depth: usize) -> Result<Value, JsonError> {
        self.budget.count_node().map_err(|kind| self.fail(kind))?;
        let probed = unsafe { probe(self.lib, x) }.map_err(|_| self.fail(JsonErrorKind::RError))?;
        match probed {
            Probe::Null => Ok(Value::Null),
            Probe::Logical(value) => {
                if value == NA_INTEGER {
                    Err(self.fail(JsonErrorKind::MissingValue))
                } else {
                    Ok(Value::Bool(value != 0))
                }
            }
            Probe::Integer(value) => {
                if value == NA_INTEGER {
                    Err(self.fail(JsonErrorKind::MissingValue))
                } else {
                    Ok(Value::Number(Number::from(value)))
                }
            }
            Probe::Real(value) => self.real(value),
            Probe::String(charsxp) => {
                if charsxp == unsafe { *self.lib.r_na_string } {
                    return Err(self.fail(JsonErrorKind::MissingValue));
                }
                Ok(Value::String(unsafe { self.string(charsxp)? }))
            }
            Probe::List { len, names } => {
                check_depth(depth + 1).map_err(|kind| self.fail(kind))?;
                match names {
                    None => unsafe { self.array(x, len, depth + 1) },
                    Some(names) => unsafe { self.object(x, len, names, depth + 1) },
                }
            }
            Probe::Reject(reject) => Err(self.reject(reject)),
        }
    }

    fn real(&self, value: f64) -> Result<Value, JsonError> {
        if value.is_nan() && value.to_bits() & 0xFFFF_FFFF == NA_REAL_PAYLOAD {
            return Err(self.fail(JsonErrorKind::MissingValue));
        }
        match Number::from_f64(value) {
            Some(number) => Ok(Value::Number(number)),
            None => Err(self.fail(JsonErrorKind::NonFiniteNumber)),
        }
    }

    /// Translate a non-NA CHARSXP to UTF-8 and charge it to the budget.
    ///
    /// # Safety
    /// `charsxp` must be a valid, reachable CHARSXP.
    unsafe fn string(&mut self, charsxp: SEXP) -> Result<String, JsonError> {
        let text =
            unsafe { charsxp_to_string(self.lib, charsxp) }.map_err(|e| self.fail(e.into()))?;
        self.budget
            .count_string(text.len())
            .map_err(|kind| self.fail(kind))?;
        Ok(text)
    }

    fn reject(&self, reject: Reject) -> JsonError {
        let kind = match reject {
            Reject::Type(code) => JsonErrorKind::UnsupportedType {
                type_name: type_name(code),
            },
            Reject::Length(length) => JsonErrorKind::InvalidLength { length },
            Reject::Class => JsonErrorKind::ClassAttribute,
            Reject::Attribute(symbol) => JsonErrorKind::UnsupportedAttribute {
                name: unsafe { self.symbol_name(symbol) },
            },
            Reject::MalformedNames => JsonErrorKind::MalformedNames,
        };
        self.fail(kind)
    }

    /// The name of a symbol, for error messages only.
    ///
    /// # Safety
    /// `symbol` must be a valid symbol.
    unsafe fn symbol_name(&self, symbol: SEXP) -> String {
        let charsxp = unsafe { (self.lib.printname)(symbol) };
        unsafe { charsxp_to_string(self.lib, charsxp) }.unwrap_or_else(|_| "?".to_string())
    }

    /// # Safety
    /// `x` must be a protected, unclassed, unnamed VECSXP of length `len`.
    unsafe fn array(&mut self, x: SEXP, len: usize, depth: usize) -> Result<Value, JsonError> {
        let mut items = Vec::with_capacity(len.min(1024));
        for index in 0..len {
            self.path.push_index(index);
            let child = unsafe { list_elt(self.lib, x, index) }
                .map_err(|_| self.fail(JsonErrorKind::RError))?;
            // SAFETY: `child` is reachable from `x`, and the guard is dropped
            // at the end of this iteration, so protections nest.
            let child = unsafe { RObject::new(child) };
            items.push(unsafe { self.convert(child.sexp(), depth)? });
            drop(child);
            self.path.pop();
        }
        Ok(Value::Array(items))
    }

    /// # Safety
    /// `x` must be a protected, unclassed VECSXP of length `len` whose
    /// validated `names` attribute is the STRSXP `names`.
    unsafe fn object(
        &mut self,
        x: SEXP,
        len: usize,
        names: SEXP,
        depth: usize,
    ) -> Result<Value, JsonError> {
        let mut members = Map::new();
        for index in 0..len {
            // While the key is being read, the member is known by position.
            self.path.push_index(index);
            let name = unsafe { string_elt(self.lib, names, index) }
                .map_err(|_| self.fail(JsonErrorKind::RError))?;
            if name == unsafe { *self.lib.r_na_string } {
                return Err(self.fail(JsonErrorKind::NaName));
            }
            let key = unsafe { self.string(name)? };
            self.path.pop();
            self.path.push_key(&key);
            if members.contains_key(&key) {
                return Err(self.fail(JsonErrorKind::DuplicateKey));
            }
            let child = unsafe { list_elt(self.lib, x, index) }
                .map_err(|_| self.fail(JsonErrorKind::RError))?;
            // SAFETY: as in `array`.
            let child = unsafe { RObject::new(child) };
            let value = unsafe { self.convert(child.sexp(), depth)? };
            drop(child);
            members.insert(key, value);
            self.path.pop();
        }
        Ok(Value::Object(members))
    }
}
