//! `.Call` routines that embedded R code uses to talk to arf.
//!
//! Both routines are registered under `PACKAGE = "(embedding)"` by
//! [`register_embedding_routines`](crate::register_embedding_routines):
//!
//! * `arf_api_version()` returns the version of this interface as an integer.
//!   R code can test for it with
//!   `is.loaded("arf_api_version", PACKAGE = "(embedding)", type = ".Call")`.
//! * `arf_publish_event(topic, data)` hands an event to the embedding
//!   application and returns whether it was queued (`TRUE`) or there was
//!   nowhere to publish it (`FALSE`). Invalid input and rejected events raise
//!   an R error.
//!
//! This crate does not know what an event bus is. The application installs a
//! publisher with [`set_event_publisher`], and the routine validates the R
//! arguments, converts `data` with [`r_to_json`](crate::json::r_to_json) and
//! calls the publisher. The publisher must not block.

use crate::ffi::top_level_exec;
use crate::json::r_to_json;
use crate::object::RObject;
use arf_libr::{CE_UTF8, R_CallMethodDef, SEXP, r_library};
use serde_json::Value;
use std::ffi::{CStr, c_void};
use std::os::raw::c_int;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

/// The version of the interface described in the module documentation.
pub const API_VERSION: i32 = 1;

/// What a publisher did with an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The event was queued for delivery. It is not a delivery guarantee: with
    /// no subscriber the event is dropped.
    Published,
    /// There is nowhere to publish: no server is running.
    NoTarget,
    /// The event was refused. The message becomes the R error message.
    Rejected(String),
}

/// Publishes an event from R code. `topic` is not validated yet; `data` is
/// the converted R value.
///
/// It runs on the R thread inside a `.Call` and must return without waiting.
pub type EventPublisher = fn(topic: &str, data: Value) -> PublishOutcome;

static PUBLISHER: OnceLock<EventPublisher> = OnceLock::new();

/// Install the publisher used by `arf_publish_event`.
///
/// The publisher is set once for the process. Returns `false`, keeping the
/// existing publisher, if one is already installed. Until one is installed,
/// `arf_publish_event` returns `FALSE`. Topic rules and size limits are the
/// publisher's to enforce, so without one they are not checked either.
pub fn set_event_publisher(publisher: EventPublisher) -> bool {
    PUBLISHER.set(publisher).is_ok()
}

pub(crate) fn publish_event_definition() -> R_CallMethodDef {
    R_CallMethodDef {
        name: c"arf_publish_event".as_ptr(),
        // SAFETY: R stores all native entries as the generic DL_FUNC ABI,
        // while `.Call` invokes this callback with the registered arity.
        fun: Some(unsafe {
            std::mem::transmute::<
                unsafe extern "C-unwind" fn(SEXP, SEXP) -> SEXP,
                unsafe extern "C" fn() -> *mut c_void,
            >(arf_publish_event)
        }),
        num_args: 2,
    }
}

pub(crate) fn api_version_definition() -> R_CallMethodDef {
    R_CallMethodDef {
        name: c"arf_api_version".as_ptr(),
        // SAFETY: as for `publish_event_definition`.
        fun: Some(unsafe {
            std::mem::transmute::<
                unsafe extern "C-unwind" fn() -> SEXP,
                unsafe extern "C" fn() -> *mut c_void,
            >(arf_api_version)
        }),
        num_args: 0,
    }
}

unsafe extern "C-unwind" fn arf_api_version() -> SEXP {
    let Ok(lib) = r_library() else {
        return std::ptr::null_mut();
    };
    unsafe { (lib.rf_scalarinteger)(API_VERSION) }
}

/// Message used when the publisher or the argument handling panics.
const PANIC_MESSAGE: &str = "publishing the event failed because of an internal error";

/// Message used if even the error message cannot be allocated in R.
const FALLBACK_MESSAGE: &CStr = c"could not publish the event (out of memory)";

unsafe extern "C-unwind" fn arf_publish_event(topic: SEXP, data: SEXP) -> SEXP {
    // Everything that owns Rust data happens inside this scope. Its result is
    // plain data, so nothing with a destructor is alive when R's C API is
    // called to build the result or to raise the error below.
    let outcome = match catch_unwind(AssertUnwindSafe(|| unsafe { publish_from_r(topic, data) })) {
        Ok(outcome) => outcome,
        // The panic payload is dropped here, before R is called again.
        Err(_) => Err(PANIC_MESSAGE.to_owned()),
    };

    let Ok(lib) = r_library() else {
        return std::ptr::null_mut();
    };
    match outcome {
        Ok(published) => unsafe { (lib.rf_scalarlogical)(c_int::from(published)) },
        Err(message) => unsafe { raise_r_error(message) },
    }
}

/// Validate the arguments, convert `data` and hand the event to the publisher.
///
/// Returns whether the event was queued, or the message of the R error to
/// raise. A missing publisher is not an error.
///
/// # Safety
/// `topic` and `data` must be valid SEXPs for the duration of the call, as
/// the arguments of a `.Call`.
unsafe fn publish_from_r(topic: SEXP, data: SEXP) -> Result<bool, String> {
    // `r_to_json` accepts exactly the values a topic may be made of, so the
    // topic goes through it as well and only has to turn out to be a string.
    let topic = match r_to_json(&unsafe { RObject::new(topic) }) {
        Ok(Value::String(topic)) => topic,
        _ => return Err("`topic` must be a single non-NA character string".to_owned()),
    };
    let data = r_to_json(&unsafe { RObject::new(data) })
        .map_err(|error| format!("`data` cannot be converted to JSON: {error}"))?;

    let Some(publisher) = PUBLISHER.get() else {
        return Ok(false);
    };
    match publisher(&topic, data) {
        PublishOutcome::Published => Ok(true),
        PublishOutcome::NoTarget => Ok(false),
        PublishOutcome::Rejected(message) => Err(message),
    }
}

/// Raise `message` as an R error, with no call.
///
/// This follows the pattern of ark's `r_unwrap`: the message is moved to the R
/// heap, every Rust value is dropped, and only then does R `longjmp` over the
/// remaining Rust frames, which therefore must not need to run destructors. The
/// protection stack is rewound by R when the error is caught, so the protected
/// message needs no unprotect.
///
/// # Safety
/// Must be called from within a `.Call`, on the R thread, with no value that
/// has a destructor alive in this or any calling Rust frame.
unsafe fn raise_r_error(message: String) -> ! {
    // The library is loaded or this routine could not be running.
    let lib = r_library().unwrap_or_else(|_| std::process::abort());
    let nil = unsafe { *lib.r_nilvalue };

    // Allocating the message can itself fail, so it runs under a top-level
    // context; the closure only touches `Copy` data. The `Result` is reduced
    // to an `Option<SEXP>` immediately, which drops any error value.
    let text = message.as_ptr();
    let length = message.len() as c_int;
    let charsxp =
        unsafe { top_level_exec(move || (lib.rf_mkcharlence)(text.cast(), length, CE_UTF8)) }.ok();

    // No R allocation happens between the creation of the CHARSXP and its
    // protection. Drop the String once its bytes are on the R heap.
    let charsxp = charsxp.map(|charsxp| unsafe { (lib.rf_protect)(charsxp) });
    drop(message);

    // The fixed format keeps `%` in the message from being interpreted.
    unsafe {
        match charsxp {
            Some(charsxp) => (lib.rf_errorcall)(nil, c"%s".as_ptr(), (lib.r_charsxp)(charsxp)),
            None => (lib.rf_errorcall)(nil, c"%s".as_ptr(), FALLBACK_MESSAGE.as_ptr()),
        }
    }
}

#[cfg(test)]
mod tests;
