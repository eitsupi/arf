//! Safe wrapper around R's SEXP objects.

mod eval;
mod misc;
mod reprex;

use crate::error::{HarpError, HarpResult};
use crate::protect::RProtect;
use arf_libr::{SEXP, SexpType, r_library, r_nil_value};

pub use eval::{EvalResult, eval_string, eval_string_in_base, eval_string_with_visibility};
pub use misc::{is_expression_complete, r_n_frame};
pub use reprex::eval_string_reprex;

/// A safe wrapper around an R SEXP object.
///
/// This struct manages the lifetime and protection of R objects.
#[derive(Debug)]
pub struct RObject {
    sexp: SEXP,
    _protect: RProtect,
}

impl RObject {
    /// Create a new RObject from a raw SEXP.
    ///
    /// # Safety
    /// The caller must ensure that `sexp` is a valid R object.
    pub unsafe fn new(sexp: SEXP) -> Self {
        let mut protect = RProtect::new();
        // SAFETY: The caller guarantees sexp is valid, and we're inside an unsafe fn
        let sexp = unsafe { protect.protect(sexp) };
        RObject {
            sexp,
            _protect: protect,
        }
    }

    /// Get the raw SEXP pointer.
    pub fn sexp(&self) -> SEXP {
        self.sexp
    }

    /// Check if this object is R's NULL.
    pub fn is_null(&self) -> bool {
        if let Ok(nil) = r_nil_value() {
            self.sexp == nil
        } else {
            false
        }
    }

    /// Get the type of this R object.
    pub fn sexp_type(&self) -> HarpResult<SexpType> {
        let lib = r_library()?;
        let type_int = unsafe { (lib.rf_typeof)(self.sexp) };
        // Convert integer to SexpType
        match type_int as u32 {
            0 => Ok(SexpType::NilSxp),
            1 => Ok(SexpType::SymSxp),
            2 => Ok(SexpType::ListSxp),
            3 => Ok(SexpType::ClosSxp),
            4 => Ok(SexpType::EnvSxp),
            5 => Ok(SexpType::PromSxp),
            6 => Ok(SexpType::LangSxp),
            7 => Ok(SexpType::SpecialSxp),
            8 => Ok(SexpType::BuiltinSxp),
            9 => Ok(SexpType::CharSxp),
            10 => Ok(SexpType::LglSxp),
            13 => Ok(SexpType::IntSxp),
            14 => Ok(SexpType::RealSxp),
            15 => Ok(SexpType::CplxSxp),
            16 => Ok(SexpType::StrSxp),
            17 => Ok(SexpType::DotSxp),
            18 => Ok(SexpType::AnySxp),
            19 => Ok(SexpType::VecSxp),
            20 => Ok(SexpType::ExprSxp),
            21 => Ok(SexpType::BcodeSxp),
            22 => Ok(SexpType::ExtptrSxp),
            23 => Ok(SexpType::WeakrefSxp),
            24 => Ok(SexpType::RawSxp),
            25 => Ok(SexpType::S4Sxp),
            _ => Err(HarpError::TypeMismatch {
                expected: "known SEXP type".to_string(),
                actual: format!("unknown type {}", type_int),
            }),
        }
    }
}
