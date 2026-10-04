//! Embedded-R integration tests for the help request bridge.

#[allow(dead_code)]
mod common;

use arf_harp::help_bridge::{
    HelpSubmitInstallOutcome, MAX_PENDING_HELP_REQUESTS, drain_prepared_help_requests,
    install_help_submit_wrapper,
};
use common::with_r;

#[test]
fn submit_help_request_prepares_atomically_and_rejects_invalid_inputs() {
    with_r!(
        submit_help_request_prepares_atomically_and_rejects_invalid_inputs,
        {
            assert!(drain_prepared_help_requests().is_empty());
            assert_eq!(
                install_help_submit_wrapper().expect("help wrapper installation should succeed"),
                HelpSubmitInstallOutcome::Installed
            );

            arf_harp::eval_string(
            r#"
mean_help <- utils::help("mean", help_type = "text")
mean_paths <- enc2utf8(unclass(mean_help))
mean_topic <- enc2utf8(attr(mean_help, "topic"))
mean_type <- enc2utf8(attr(mean_help, "type"))
mean_tried <- attr(mean_help, "tried_all_packages")
sum_help <- utils::help("sum", help_type = "text")
sum_paths <- enc2utf8(unclass(sum_help))
submit_help <- function(paths = mean_paths, topic = mean_topic, type = mean_type, tried = mean_tried) {
  .Call("arf_submit_help_request", paths, topic, type, tried, PACKAGE = "(embedding)")
}
stopifnot(identical(submit_help(), TRUE))
stopifnot(identical(submit_help(c(mean_paths, sum_paths)), TRUE))

stopifnot(identical(submit_help(paths = 42L), FALSE))
stopifnot(identical(submit_help(paths = character()), FALSE))
stopifnot(identical(submit_help(paths = NA_character_), FALSE))
stopifnot(identical(submit_help(topic = NA_character_), FALSE))
stopifnot(identical(submit_help(topic = 42L), FALSE))
stopifnot(identical(submit_help(topic = c(mean_topic, "other")), FALSE))
stopifnot(identical(submit_help(type = NA_character_), FALSE))
stopifnot(identical(submit_help(type = "html"), FALSE))
stopifnot(identical(submit_help(type = "pdf"), FALSE))
stopifnot(identical(submit_help(type = 42L), FALSE))
stopifnot(identical(submit_help(tried = TRUE), FALSE))
stopifnot(identical(submit_help(tried = NA), FALSE))
stopifnot(identical(submit_help(tried = logical()), FALSE))
stopifnot(identical(submit_help(tried = "FALSE"), FALSE))

missing_path <- file.path(dirname(dirname(mean_paths[[1L]])), "help", "missing-topic")
stopifnot(identical(submit_help(paths = missing_path), FALSE))
stopifnot(identical(submit_help(paths = c(mean_paths, missing_path)), FALSE))
traversal_path <- file.path(dirname(dirname(mean_paths[[1L]])), "help", "..", "help", "mean")
stopifnot(identical(submit_help(paths = traversal_path), FALSE))
"#,
        )
        .expect("R should accept valid requests and reject invalid requests");

            let requests = drain_prepared_help_requests();
            assert_eq!(
                requests.len(),
                2,
                "rejections and partial failure must not enqueue"
            );

            let first = &requests[0];
            assert_eq!(first.topic, "mean");
            assert_eq!(first.pages.len(), 1);
            assert_eq!(first.pages[0].package, "base");
            assert_eq!(first.pages[0].display_topic, "mean");
            assert_eq!(first.pages[0].help_key, "mean");
            assert!(!first.pages[0].markdown.is_empty());
            assert!(first.pages[0].package_dir.ends_with("base"));

            let second = &requests[1];
            assert_eq!(second.topic, "mean");
            assert_eq!(
                second
                    .pages
                    .iter()
                    .map(|page| page.help_key.as_str())
                    .collect::<Vec<_>>(),
                ["mean", "sum"]
            );

            assert_eq!(MAX_PENDING_HELP_REQUESTS, 16);
        }
    );
}
