//! Tests for registering a complete native call-method table.

use crate::test_support::with_r_in_subprocess;
use arf_libr::{R_CallMethodDef, R_FALSE, R_TRUE, SEXP, SexpType, r_library};
use std::ffi::CStr;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

pub(super) static REGISTRATION_CALLS: AtomicUsize = AtomicUsize::new(0);

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
    .unwrap_or(std::ptr::null_mut())
}

fn take_callback_outcome() -> CallbackOutcome {
    CALLBACK_OUTCOME
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .expect("callback should have recorded an outcome")
}

unsafe extern "C" fn return_true() -> SEXP {
    let lib = r_library().expect("R should be loaded");
    unsafe { (lib.rf_scalarlogical)(R_TRUE) }
}

#[test]
fn register_and_call_two_embedded_routines() {
    register_and_call_two_embedded_routines_in_r();
}

#[test]
#[ignore = "runs in an isolated R subprocess via the test above"]
fn register_and_call_two_embedded_routines_in_r() {
    with_r_in_subprocess(
        concat!(
            module_path!(),
            "::register_and_call_two_embedded_routines_in_r"
        ),
        || unsafe {
            let definition = R_CallMethodDef {
                name: c"arf_harp_test_capture".as_ptr(),
                fun: Some(std::mem::transmute::<
                    unsafe extern "C" fn(SEXP) -> SEXP,
                    unsafe extern "C" fn() -> *mut c_void,
                >(capture_character_vector)),
                num_args: 1,
            };

            let second_definition = R_CallMethodDef {
                name: c"arf_harp_test_true".as_ptr(),
                fun: Some(std::mem::transmute::<
                    unsafe extern "C" fn() -> SEXP,
                    unsafe extern "C" fn() -> *mut c_void,
                >(return_true)),
                num_args: 0,
            };
            super::register_complete_call_table(&[definition, second_definition])
                .expect("one complete table should register both routines");

            crate::eval_string(
                r#"
stopifnot(identical(.Call("arf_harp_test_true", PACKAGE = "(embedding)"), TRUE))
stopifnot(identical(names(getDLLRegisteredRoutines("(embedding)")[[".Call"]]),
                    c("arf_harp_test_capture", "arf_harp_test_true")))
"#,
            )
            .expect("both callbacks should be registered in the same table");

            crate::eval_string(
                r#"
stopifnot(identical(.Call("arf_harp_test_capture", c("first", "second"),
                        PACKAGE = "(embedding)"), TRUE))
"#,
            )
            .expect("registered callback should accept a character vector");
            assert_eq!(
                take_callback_outcome(),
                CallbackOutcome::Accepted(vec!["first".to_string(), "second".to_string()])
            );

            crate::eval_string(
                r#"
stopifnot(identical(.Call("arf_harp_test_capture", 42L, PACKAGE = "(embedding)"), FALSE))
"#,
            )
            .expect("callback should safely reject a non-character vector");
            assert_eq!(take_callback_outcome(), CallbackOutcome::RejectedType(13));

            crate::eval_string(
                r#"
stopifnot(identical(.Call("arf_harp_test_capture", "panic", PACKAGE = "(embedding)"), FALSE))
"#,
            )
            .expect("callback panic should be contained at the native boundary");
            assert_eq!(take_callback_outcome(), CallbackOutcome::RejectedPanic);
        },
    );
}

#[test]
fn embedding_registration_calls_r_only_once() {
    embedding_registration_calls_r_only_once_in_r();
}

#[test]
#[ignore = "runs in an isolated R subprocess via the test above"]
fn embedding_registration_calls_r_only_once_in_r() {
    with_r_in_subprocess(
        concat!(
            module_path!(),
            "::embedding_registration_calls_r_only_once_in_r"
        ),
        || {
            assert_eq!(REGISTRATION_CALLS.load(Ordering::Relaxed), 0);
            super::register_embedding_routines().expect("initial registration should succeed");
            assert_eq!(REGISTRATION_CALLS.load(Ordering::Relaxed), 1);
            crate::eval_string(
                r#"
registered_before <- getDLLRegisteredRoutines("(embedding)")[[".Call"]]
registered_before[["arf_submit_help_request"]]$address <- NULL
stopifnot(identical(names(registered_before), "arf_submit_help_request"))
stopifnot(identical(registered_before[["arf_submit_help_request"]]$numParameters, 4L))
"#,
            )
            .expect("the complete production table should contain the help callback");
            super::register_embedding_routines().expect("repeat registration should succeed");
            assert_eq!(REGISTRATION_CALLS.load(Ordering::Relaxed), 1);
            crate::eval_string(
                r#"
registered_after <- getDLLRegisteredRoutines("(embedding)")[[".Call"]]
registered_after[["arf_submit_help_request"]]$address <- NULL
stopifnot(identical(registered_before, registered_after))
stopifnot(identical(.Call("arf_submit_help_request", character(), "mean", "text", FALSE,
                        PACKAGE = "(embedding)"), FALSE))
"#,
            )
            .expect("registration should retain the routine metadata and callable callback");

            crate::help_bridge::install_help_submit_wrapper()
                .expect("wrapper installation should succeed");
            crate::eval_string(
                r#"
ns <- asNamespace("utils")
standard <- get("print.help_files_with_topic", envir = ns, inherits = FALSE)
registerS3method("print", "help_files_with_topic", standard, envir = ns)
"#,
            )
            .expect("R should restore the standard method");
            assert_eq!(
                crate::help_bridge::install_help_submit_wrapper()
                    .expect("wrapper reinstallation should succeed"),
                crate::help_bridge::HelpSubmitInstallOutcome::Installed
            );
            assert_eq!(REGISTRATION_CALLS.load(Ordering::Relaxed), 1);
        },
    );
}
