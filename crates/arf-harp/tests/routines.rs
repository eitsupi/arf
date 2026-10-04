//! Integration tests for arf's one-time embedding routine registration.

#[allow(dead_code)]
mod common;

use arf_harp::routines::register_embedding_routines;
use common::with_r;

#[test]
fn embedding_callbacks_remain_callable_on_repeat_registration() {
    with_r(|| {
        register_embedding_routines().expect("embedding registration should succeed");
        arf_harp::eval_string(
            r#"
registered_before <- getDLLRegisteredRoutines("(embedding)")[[".Call"]]
registered_before[["arf_submit_help_request"]]$address <- NULL
stopifnot(identical(names(registered_before), "arf_submit_help_request"))
stopifnot(identical(registered_before[["arf_submit_help_request"]]$numParameters, 4L))
"#,
        )
        .expect("the complete production table should contain the help callback");

        register_embedding_routines().expect("repeated registration should be a no-op");
        arf_harp::eval_string(
            r#"
registered_after <- getDLLRegisteredRoutines("(embedding)")[[".Call"]]
registered_after[["arf_submit_help_request"]]$address <- NULL
stopifnot(identical(registered_before, registered_after))
stopifnot(identical(.Call("arf_submit_help_request", character(), "mean", "text", FALSE,
                        PACKAGE = "(embedding)"), FALSE))
"#,
        )
        .expect("registration should retain the routine metadata and callable callback");
    });
}
