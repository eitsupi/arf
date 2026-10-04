//! Embedded-R tests for conditional S3 help-method installation.

#[allow(dead_code)]
mod common;

use arf_harp::help_bridge::{
    HelpSubmitInstallOutcome, drain_prepared_help_requests, install_help_submit_wrapper,
};
use common::with_r;

#[test]
fn installer_preserves_custom_methods_and_wraps_only_the_standard_method() {
    with_r!(
        installer_preserves_custom_methods_and_wraps_only_the_standard_method,
        {
            assert!(drain_prepared_help_requests().is_empty());
            arf_harp::eval_string(
            r#"
utils_ns <- asNamespace("utils")
standard_help_printer <- get("print.help_files_with_topic", envir = utils_ns, inherits = FALSE)
registered_help_printer <- getS3method("print", "help_files_with_topic", optional = TRUE, envir = utils_ns)
stopifnot(identical(registered_help_printer, standard_help_printer))

custom_help_called <- FALSE
custom_help_printer <- function(x, ...) {
  assign("custom_help_called", TRUE, envir = .GlobalEnv)
  invisible(x)
}
registerS3method("print", "help_files_with_topic", custom_help_printer, envir = utils_ns)
"#,
        )
        .expect("R should install the pre-existing custom printer");

            assert_eq!(
                install_help_submit_wrapper().expect("custom printer check should succeed"),
                HelpSubmitInstallOutcome::SkippedExistingMethod
            );
            arf_harp::eval_string(
                r#"
print(utils::help("mean", help_type = "text"))
stopifnot(isTRUE(custom_help_called))
registerS3method("print", "help_files_with_topic", standard_help_printer, envir = utils_ns)
"#,
            )
            .expect("the pre-existing custom printer should remain active");
            assert!(drain_prepared_help_requests().is_empty());

            assert_eq!(
                install_help_submit_wrapper().expect("standard printer install should succeed"),
                HelpSubmitInstallOutcome::Installed
            );
            assert_eq!(
                install_help_submit_wrapper().expect("repeated install should safely skip"),
                HelpSubmitInstallOutcome::SkippedExistingMethod
            );

            arf_harp::eval_string(
                r#"
embedding_routines_before <- getDLLRegisteredRoutines("(embedding)")[[".Call"]]
embedding_routines_before[["arf_submit_help_request"]]$address <- NULL
registerS3method("print", "help_files_with_topic", standard_help_printer, envir = utils_ns)
"#,
            )
            .expect("R should restore the standard printer before reinstallation");
            assert_eq!(
                install_help_submit_wrapper().expect("wrapper reinstallation should succeed"),
                HelpSubmitInstallOutcome::Installed
            );
            arf_harp::eval_string(
                r#"
embedding_routines_after <- getDLLRegisteredRoutines("(embedding)")[[".Call"]]
embedding_routines_after[["arf_submit_help_request"]]$address <- NULL
stopifnot(identical(embedding_routines_before, embedding_routines_after))
"#,
            )
            .expect("reinstallation must preserve native routine metadata");

            arf_harp::eval_string(
                r#"
assigned_help <- utils::help("mean", help_type = "text")
"#,
            )
            .expect("assigning a help object should succeed");
            assert!(drain_prepared_help_requests().is_empty());

            arf_harp::eval_string(
                r#"
invisible(print(assigned_help))
invisible(print(utils::help("lm", package = "stats", help_type = "text")))
"#,
            )
            .expect("standard and package-qualified help should be printable");
            let requests = drain_prepared_help_requests();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].topic, "mean");
            assert_eq!(requests[0].pages[0].package, "base");
            assert_eq!(requests[0].pages[0].help_key, "mean");
            assert_eq!(requests[1].topic, "lm");
            assert!(requests[1].pages.iter().any(|page| page.package == "stats"));

            arf_harp::eval_string(
                r#"
bad_help <- utils::help("mean", help_type = "text")
attr(bad_help, "tried_all_packages") <- TRUE
fallback_output <- utils::capture.output(print(bad_help))
stopifnot(length(fallback_output) > 0L)
"#,
            )
            .expect("a rejected submit should fall back to the standard R printer");
            assert!(drain_prepared_help_requests().is_empty());

            arf_harp::eval_string(
                r#"
missing_help <- utils::help("arf_no_such_topic_7a81", package = "utils", help_type = "text")
stopifnot(length(missing_help) == 0L)
missing_output <- utils::capture.output(print(missing_help))
stopifnot(length(missing_output) > 0L)
"#,
            )
            .expect("a missing topic should use R's normal no-documentation fallback");
            assert!(drain_prepared_help_requests().is_empty());

            arf_harp::eval_string(
                r#"
pdf_help <- utils::help("mean", help_type = "text")
attr(pdf_help, "type") <- "pdf"
stopifnot(identical(attr(pdf_help, "tried_all_packages"), FALSE))
pdf_wrapper <- getS3method("print", "help_files_with_topic")
pdf_wrapper_env <- environment(pdf_wrapper)
stopifnot(exists("fallback", envir = pdf_wrapper_env, inherits = FALSE))
original_pdf_fallback <- get("fallback", envir = pdf_wrapper_env, inherits = FALSE)
pdf_fallback_called <- FALSE
assign("fallback", function(x, ...) {
  assign("pdf_fallback_called", TRUE, envir = .GlobalEnv)
  invisible(x)
}, envir = pdf_wrapper_env)
tryCatch({
  print(pdf_help)
  stopifnot(isTRUE(pdf_fallback_called))
}, finally = {
  assign("fallback", original_pdf_fallback, envir = pdf_wrapper_env)
})
"#,
            )
            .expect("PDF alone should reject through the wrapper and use its fallback binding");
            assert!(drain_prepared_help_requests().is_empty());

            arf_harp::eval_string(
                r#"
late_custom_called <- FALSE
late_custom_printer <- function(x, ...) {
  assign("late_custom_called", TRUE, envir = .GlobalEnv)
  invisible(x)
}
registerS3method("print", "help_files_with_topic", late_custom_printer, envir = utils_ns)
print(utils::help("mean", help_type = "text"))
stopifnot(isTRUE(late_custom_called))
"#,
            )
            .expect("a late custom method should win S3 dispatch");
            assert!(drain_prepared_help_requests().is_empty());
        }
    );
}
