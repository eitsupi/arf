//! R handlers explicitly exposed to IPC clients by a named function list.
//!
//! A registry belongs to one R thread. Functions are retained by R, while
//! names and the handler index are ordinary Rust data. Exposing a list is
//! atomic: it is fully evaluated and validated before any registry state is
//! changed.

use crate::object::RObject;
use crate::{
    ffi::top_level_exec,
    json::{JsonError, json_to_r, r_to_json},
};
use arf_libr::{
    ParseStatus, RLibrary, SEXP, SexpType, finish_ipc_capture, r_library, r_nil_value,
    start_ipc_capture,
};
use serde_json::Value;
use std::ffi::CString;
use std::marker::PhantomData;
use std::rc::Rc;

const CALL_WRAPPER: &str = r#"function(fun, args) {
  condition_message <- function(e, fallback) {
    base::tryCatch({
      message <- base::conditionMessage(e)
      if (base::is.character(message) && base::length(message) == 1L && !base::is.na(message)) {
        message
      } else {
        fallback
      }
    }, error = function(ignored) fallback)
  }
  base::tryCatch(
    base::withCallingHandlers(
      base::list(TRUE, fun(args)),
      warning = function(w) {
        if (base::getOption("warn") == 0L) {
          base::message("Warning: ", condition_message(w, "R warning"))
          base::invokeRestart("muffleWarning")
        }
      }
    ),
    error = function(e) base::list(FALSE, condition_message(e, "R error"),
                                   base::as.list(base::class(e)), FALSE),
    interrupt = function(e) base::list(FALSE, condition_message(e, "Interrupted"),
                                       base::as.list(base::class(e)), TRUE)
  )
}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerErrorKind {
    ArgumentConversion,
    NotFound,
    RError,
    ReturnConversion,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerError {
    pub kind: HandlerErrorKind,
    pub message: String,
    pub class: Vec<String>,
    pub stdout: String,
    pub stderr: String,
}

impl HandlerError {
    pub fn new(kind: HandlerErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            class: Vec::new(),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    pub fn not_found(handler: &str) -> Self {
        Self::new(
            HandlerErrorKind::NotFound,
            format!("Handler {handler:?} is not published"),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandlerResult {
    pub value: Value,
    pub stdout: String,
    pub stderr: String,
}

struct Handler {
    name: String,
    function: SEXP,
}

struct IpcCapture {
    active: bool,
}

impl IpcCapture {
    fn start() -> Self {
        start_ipc_capture(false);
        Self { active: true }
    }

    fn finish(mut self) -> (String, String) {
        self.active = false;
        finish_ipc_capture()
    }
}

impl Drop for IpcCapture {
    fn drop(&mut self) {
        if self.active {
            let _ = finish_ipc_capture();
        }
    }
}

/// A thread-affine collection of R functions published for IPC calls.
///
/// The R objects in this registry are rooted with `R_PreserveObject`; they do
/// not occupy R's protection stack between calls. Create, use, and drop the
/// registry on the R thread.
pub struct HandlerRegistry {
    root: Option<SEXP>,
    call_wrapper: SEXP,
    handlers: Vec<Handler>,
    revoked: bool,
    _not_send: PhantomData<Rc<()>>,
}

impl HandlerRegistry {
    /// Prepare a trusted base-environment wrapper for calls.
    pub fn new() -> Result<Self, String> {
        let lib = r_library().map_err(|e| e.to_string())?;
        let capture = IpcCapture::start();
        let base = unsafe { *lib.r_baseenv };
        let wrapper_result = evaluate_without_print(CALL_WRAPPER, base);
        let _ = capture.finish();
        let wrapper = wrapper_result?;
        if !is_function(unsafe { (lib.rf_typeof)(wrapper.sexp()) }) {
            return Err("Failed to create the internal handler call wrapper".to_owned());
        }
        let wrapper_sexp = wrapper.sexp();
        // Keep the wrapper alive across calls, without leaving an R PROTECT
        // entry on the stack for the lifetime of the registry.
        unsafe { top_level_exec(move || (lib.r_preserve_object)(wrapper_sexp)) }
            .map_err(|error| format!("Could not retain the handler call wrapper: {error}"))?;
        Ok(Self {
            root: None,
            call_wrapper: wrapper_sexp,
            handlers: Vec::new(),
            revoked: false,
            _not_send: PhantomData,
        })
    }

    pub fn is_locked(&self) -> bool {
        self.root.is_some() && !self.revoked
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked
    }

    pub fn names(&self) -> Vec<String> {
        self.handlers
            .iter()
            .map(|handler| handler.name.clone())
            .collect()
    }

    /// Evaluate `expression`, validate its entire named function list, and
    /// publish it in one step. An empty list is valid and locks the registry.
    pub fn expose(&mut self, expression: &str) -> Result<Vec<String>, String> {
        if self.revoked {
            return Err("Handler publication has been revoked".to_owned());
        }
        if self.root.is_some() {
            return Err("Handlers have already been published".to_owned());
        }
        let lib = r_library().map_err(|e| e.to_string())?;
        let global = unsafe { *lib.r_globalenv };
        let list = evaluate_without_print(expression, global)
            .map_err(|error| format!("Could not evaluate the handler expression: {error}"))?;
        if unsafe { (lib.rf_typeof)(list.sexp()) } != SexpType::VecSxp as i32 {
            return Err("The handler expression must return a named list of functions".to_owned());
        }

        let len = unsafe { (lib.rf_xlength)(list.sexp()).max(0) as usize };
        let names = unsafe { (lib.rf_getattrib)(list.sexp(), *lib.r_namessymbol) };
        if names == r_nil_value().map_err(|e| e.to_string())? {
            return Err("The handler list must have names".to_owned());
        }
        if unsafe { (lib.rf_typeof)(names) } != SexpType::StrSxp as i32
            || unsafe { (lib.rf_xlength)(names).max(0) as usize } != len
        {
            return Err("The handler list has malformed names".to_owned());
        }
        // Protect the names attribute for the loop. The parent list already
        // roots each function while the list is inspected.
        let names = unsafe { RObject::new(names) };
        // Build a holder while validating. Every function is inserted as soon
        // as it is fetched, so a temporary element from an ALTREP list cannot
        // be collected before the final retained root is established.
        let holder = unsafe {
            top_level_exec(move || (lib.rf_allocvector)(SexpType::VecSxp as i32, len as isize))
        }
        .map_err(|e| format!("Could not allocate the handler root: {e}"))?;
        let holder = unsafe { RObject::new(holder) };
        let mut handlers = Vec::with_capacity(len);
        for index in 0..len {
            let name_sexp = string_elt(lib, names.sexp(), index)
                .map_err(|e| format!("Could not read handler name at index {index}: {e}"))?;
            let name = scalar_string(lib, name_sexp)
                .map_err(|error| format!("Invalid handler name at index {index}: {error}"))?;
            validate_handler_name(&name)?;
            if handlers
                .iter()
                .any(|handler: &Handler| handler.name == name)
            {
                return Err(format!("Duplicate handler name {name:?}"));
            }
            let function = vector_elt(lib, list.sexp(), index)
                .map_err(|e| format!("Could not read handler {name:?}: {e}"))?;
            let function_type = unsafe { (lib.rf_typeof)(function) };
            if !is_function(function_type) {
                return Err(format!("Handler {name:?} must be a function"));
            }
            unsafe { (lib.set_vector_elt)(holder.sexp(), index as isize, function) };
            handlers.push(Handler { name, function });
        }

        let root = holder.sexp();
        let root_for_preserve = root;
        unsafe {
            top_level_exec(move || {
                // Preserve while the holder is still on the protection stack;
                // R_PreserveObject may allocate and trigger a collection.
                (lib.r_preserve_object)(root_for_preserve);
            })
        }
        .map_err(|e| format!("Could not retain the published handlers: {e}"))?;
        self.root = Some(root);
        self.handlers = handlers;
        Ok(self.names())
    }

    /// Permanently disable this registry and release its retained functions.
    pub fn revoke(&mut self) -> Vec<String> {
        let names = self.names();
        self.release_root();
        self.handlers.clear();
        self.revoked = true;
        names
    }

    /// Call a published function with one R argument created from `args`.
    pub fn call(&self, name: &str, args: &Value) -> Result<HandlerResult, HandlerError> {
        if self.revoked || self.root.is_none() {
            return Err(HandlerError::not_found(name));
        }
        let Some(handler) = self.handlers.iter().find(|handler| handler.name == name) else {
            return Err(HandlerError::not_found(name));
        };
        let lib =
            r_library().map_err(|e| HandlerError::new(HandlerErrorKind::RError, e.to_string()))?;
        let capture = IpcCapture::start();
        let mut result = (|| {
            let argument = json_to_r(args).map_err(|error| {
                HandlerError::new(HandlerErrorKind::ArgumentConversion, error.to_string())
            })?;
            let call_wrapper = self.call_wrapper;
            let function = handler.function;
            let argument = argument.sexp();
            let call_result = unsafe {
                top_level_exec(move || {
                    let nil = *lib.r_nilvalue;
                    let args = (lib.rf_protect)((lib.rf_cons)(argument, nil));
                    let function_arg = (lib.rf_protect)((lib.rf_cons)(function, args));
                    let call = (lib.rf_protect)((lib.rf_lcons)(call_wrapper, function_arg));
                    let result = (lib.rf_eval)(call, *lib.r_baseenv);
                    (lib.rf_unprotect)(3);
                    result
                })
            };
            let result = call_result.map_err(|error| {
                HandlerError::new(
                    // User interrupts raised during the handler are returned
                    // through the wrapper's explicit interrupt branch. An
                    // error escaping the wrapper is an R-side failure (for
                    // example, a failing condition formatter), not evidence
                    // of an interrupt.
                    HandlerErrorKind::RError,
                    format!("R handler evaluation failed outside its condition handler: {error}"),
                )
            })?;
            let result = unsafe { RObject::new(result) };
            let outcome = extract_call_outcome(&result)
                .map_err(|message| HandlerError::new(HandlerErrorKind::RError, message))?;
            match outcome {
                CallOutcome::Success(value) => {
                    let value = r_to_json(&value).map_err(|error| {
                        HandlerError::new(HandlerErrorKind::ReturnConversion, error.to_string())
                    })?;
                    Ok(HandlerResult {
                        value,
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                }
                CallOutcome::Error {
                    message,
                    class,
                    interrupted,
                } => {
                    let mut error = HandlerError::new(
                        if interrupted {
                            HandlerErrorKind::Interrupted
                        } else {
                            HandlerErrorKind::RError
                        },
                        message,
                    );
                    error.class = class;
                    Err(error)
                }
            }
        })();
        let (stdout, stderr) = capture.finish();
        match &mut result {
            Ok(response) => {
                response.stdout = stdout;
                response.stderr = stderr;
            }
            Err(error) => {
                error.stdout = stdout;
                error.stderr = stderr;
            }
        }
        result
    }

    fn release_root(&mut self) {
        let Ok(lib) = r_library() else { return };
        if let Some(root) = self.root.take() {
            unsafe { (lib.r_release_object)(root) };
        }
    }
}

impl Drop for HandlerRegistry {
    fn drop(&mut self) {
        self.release_root();
        if let Ok(lib) = r_library() {
            unsafe { (lib.r_release_object)(self.call_wrapper) };
        }
    }
}

enum CallOutcome {
    Success(RObject),
    Error {
        message: String,
        class: Vec<String>,
        interrupted: bool,
    },
}

fn extract_call_outcome(result: &RObject) -> Result<CallOutcome, String> {
    let lib = r_library().map_err(|e| e.to_string())?;
    if unsafe { (lib.rf_typeof)(result.sexp()) } != SexpType::VecSxp as i32
        || unsafe { (lib.rf_xlength)(result.sexp()) } < 2
    {
        return Err("The internal handler call wrapper returned a malformed result".to_owned());
    }
    let success = vector_elt(lib, result.sexp(), 0).map_err(|e| e.to_string())?;
    if unsafe { (lib.rf_typeof)(success) } != SexpType::LglSxp as i32
        || unsafe { (lib.rf_xlength)(success) } != 1
    {
        return Err("The internal handler call wrapper returned an invalid status".to_owned());
    }
    let success = unsafe { *(lib.logical)(success) != 0 };
    let value = vector_elt(lib, result.sexp(), 1).map_err(|e| e.to_string())?;
    if success {
        return Ok(CallOutcome::Success(unsafe { RObject::new(value) }));
    }
    if unsafe { (lib.rf_xlength)(result.sexp()) } != 4 {
        return Err("The internal handler call wrapper returned a malformed error".to_owned());
    }
    let message = scalar_string(lib, value).map_err(|e| e.to_string())?;
    let classes = vector_elt(lib, result.sexp(), 2).map_err(|e| e.to_string())?;
    let class_len = unsafe { (lib.rf_xlength)(classes).max(0) as usize };
    let mut class = Vec::with_capacity(class_len);
    for index in 0..class_len {
        let class_sexp = vector_elt(lib, classes, index).map_err(|e| e.to_string())?;
        class.push(scalar_string(lib, class_sexp).map_err(|e| e.to_string())?);
    }
    let interrupted = vector_elt(lib, result.sexp(), 3).map_err(|e| e.to_string())?;
    if unsafe { (lib.rf_typeof)(interrupted) } != SexpType::LglSxp as i32 {
        return Err(
            "The internal handler call wrapper returned an invalid interrupt status".to_owned(),
        );
    }
    Ok(CallOutcome::Error {
        message,
        class,
        interrupted: unsafe { *(lib.logical)(interrupted) != 0 },
    })
}

fn scalar_string(lib: &'static RLibrary, sexp: SEXP) -> Result<String, JsonError> {
    // r_to_json handles STRSXP. Wrap a CHARSXP from STRING_ELT first.
    let chars_root = if unsafe { (lib.rf_typeof)(sexp) } == SexpType::CharSxp as i32 {
        // STRING_ELT may return a temporary CHARSXP for ALTREP strings. Root
        // it before allocating the one-element STRSXP below.
        Some(unsafe { RObject::new(sexp) })
    } else {
        None
    };
    let string = if unsafe { (lib.rf_typeof)(sexp) } == SexpType::StrSxp as i32 {
        sexp
    } else if chars_root.is_some() {
        unsafe {
            top_level_exec(move || {
                let vector = (lib.rf_protect)((lib.rf_allocvector)(SexpType::StrSxp as i32, 1));
                (lib.set_string_elt)(vector, 0, sexp);
                (lib.rf_unprotect)(1);
                vector
            })
        }
        .map_err(|_| JsonError::at_root(crate::json::JsonErrorKind::MissingValue))?
    } else {
        return Err(JsonError::at_root(crate::json::JsonErrorKind::MissingValue));
    };
    let object = unsafe { RObject::new(string) };
    let result = match r_to_json(&object)? {
        Value::String(value) => Ok(value),
        _ => Err(crate::json::JsonError::at_root(
            crate::json::JsonErrorKind::MissingValue,
        )),
    };
    drop(object);
    drop(chars_root);
    result
}

/// Parse and evaluate source without printing visible expression results.
///
/// Keep parsing and evaluation in one `R_ToplevelExec` closure so every R
/// object stays protected while it is in use. The closure only captures and
/// returns `Copy` values; its protection stack is balanced before returning.
fn evaluate_without_print(code: &str, env: SEXP) -> Result<RObject, String> {
    let lib = r_library().map_err(|error| error.to_string())?;
    let code = CString::new(code).map_err(|_| "R source contains a NUL byte".to_owned())?;
    let code_ptr = code.as_ptr();
    let mut status = ParseStatus::Null;
    let status_ptr = &mut status as *mut ParseStatus;
    let status_out = status_ptr;
    let value = unsafe {
        top_level_exec(move || {
            let code_sexp = (lib.rf_protect)((lib.rf_mkstring)(code_ptr));
            let parsed = (lib.rf_protect)((lib.r_parsevector)(
                code_sexp,
                -1,
                status_out,
                *lib.r_nilvalue,
            ));
            let mut value = *lib.r_nilvalue;
            if *status_out == ParseStatus::Ok {
                let expression_count = (lib.rf_length)(parsed);
                for index in 0..expression_count as isize {
                    let expression = (lib.vector_elt)(parsed, index);
                    value = (lib.rf_eval)(expression, env);
                }
            }
            (lib.rf_unprotect)(2);
            value
        })
    }
    .map_err(|error| format!("R evaluation failed: {error}"))?;

    match status {
        ParseStatus::Ok => Ok(unsafe { RObject::new(value) }),
        ParseStatus::Incomplete => Err("Incomplete R expression".to_owned()),
        ParseStatus::Error => Err("R parse error".to_owned()),
        other => Err(format!("Unexpected R parse status: {other:?}")),
    }
}

fn string_elt(lib: &'static RLibrary, strings: SEXP, index: usize) -> Result<SEXP, String> {
    unsafe { top_level_exec(move || (lib.string_elt)(strings, index as isize)) }
        .map_err(|e| e.to_string())
}

fn vector_elt(lib: &'static RLibrary, list: SEXP, index: usize) -> Result<SEXP, String> {
    unsafe { top_level_exec(move || (lib.vector_elt)(list, index as isize)) }
        .map_err(|e| e.to_string())
}

fn is_function(sexp_type: i32) -> bool {
    sexp_type == SexpType::ClosSxp as i32
        || sexp_type == SexpType::BuiltinSxp as i32
        || sexp_type == SexpType::SpecialSxp as i32
}

fn validate_handler_name(name: &str) -> Result<(), String> {
    let Some((namespace, method)) = name.split_once('/') else {
        return Err(format!(
            "Handler name {name:?} must be <namespace>/<method>"
        ));
    };
    if namespace.is_empty() || method.is_empty() || method.contains('/') || name.starts_with("arf/")
    {
        return Err(format!("Invalid or reserved handler name {name:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
