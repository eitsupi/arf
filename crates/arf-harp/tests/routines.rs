//! Integration tests for embedded R call-method registration.

#[allow(dead_code)]
mod common;

use arf_libr::{R_CallMethodDef, R_FALSE, R_TRUE, SEXP, SexpType, r_library};
use common::with_r;
use std::ffi::CStr;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, PartialEq, Eq)]
enum CallbackOutcome {
    Accepted(Vec<String>),
    RejectedType(i32),
    RejectedPanic,
}

static CALLBACK_OUTCOME: OnceLock<Mutex<Option<CallbackOutcome>>> = OnceLock::new();

unsafe extern "C" fn capture_character_vector(input: SEXP) -> SEXP {
    catch_unwind(AssertUnwindSafe(|| unsafe {
        let outcome = match catch_unwind(AssertUnwindSafe(|| {
            if input.is_null() {
                return CallbackOutcome::RejectedType(-1);
            }
            let Ok(lib) = r_library() else {
                return CallbackOutcome::RejectedType(-2);
            };
            let actual_type = (lib.rf_typeof)(input);
            if actual_type != SexpType::StrSxp as i32 {
                return CallbackOutcome::RejectedType(actual_type);
            }

            let length = (lib.rf_length)(input).max(0);
            let mut values = Vec::with_capacity(length as usize);
            for index in 0..length as isize {
                let element = (lib.string_elt)(input, index);
                let chars = (lib.r_charsxp)(element);
                if chars.is_null() {
                    return CallbackOutcome::RejectedType(-3);
                }
                let text = CStr::from_ptr(chars).to_string_lossy().into_owned();
                if text == "panic" {
                    panic!("test callback panic");
                }
                values.push(text);
            }
            CallbackOutcome::Accepted(values)
        })) {
            Ok(outcome) => outcome,
            Err(_) => CallbackOutcome::RejectedPanic,
        };

        let accepted = matches!(outcome, CallbackOutcome::Accepted(_));
        *CALLBACK_OUTCOME
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);

        let Ok(lib) = r_library() else {
            return std::ptr::null_mut();
        };
        (lib.rf_scalarlogical)(if accepted { R_TRUE } else { R_FALSE })
    }))
    .unwrap_or_else(|_| std::ptr::null_mut())
}

fn take_callback_outcome() -> CallbackOutcome {
    CALLBACK_OUTCOME
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .expect("callback should have recorded an outcome")
}

#[test]
fn register_and_call_embedded_routine() {
    with_r(|| unsafe {
        let definition = R_CallMethodDef {
            name: c"arf_harp_test_capture".as_ptr(),
            fun: Some(std::mem::transmute::<
                unsafe extern "C" fn(SEXP) -> SEXP,
                unsafe extern "C" fn() -> *mut c_void,
            >(capture_character_vector)),
            num_args: 1,
        };

        arf_harp::routines::register_call_methods(&[definition])
            .expect("first registration should succeed");
        arf_harp::routines::register_call_methods(&[definition])
            .expect("same-name re-registration should succeed safely");

        arf_harp::eval_string(
            r#"stopifnot(identical(.Call("arf_harp_test_capture", c("first", "second"), PACKAGE = "(embedding)"), TRUE))"#,
        )
        .expect("registered callback should accept a character vector");
        assert_eq!(
            take_callback_outcome(),
            CallbackOutcome::Accepted(vec!["first".to_string(), "second".to_string()])
        );

        arf_harp::eval_string(
            r#"stopifnot(identical(.Call("arf_harp_test_capture", 42L, PACKAGE = "(embedding)"), FALSE))"#,
        )
        .expect("callback should safely reject a non-character vector");
        assert_eq!(take_callback_outcome(), CallbackOutcome::RejectedType(13));

        arf_harp::eval_string(
            r#"stopifnot(identical(.Call("arf_harp_test_capture", "panic", PACKAGE = "(embedding)"), FALSE))"#,
        )
        .expect("callback panic should be contained at the native boundary");
        assert_eq!(take_callback_outcome(), CallbackOutcome::RejectedPanic);
    });
}
