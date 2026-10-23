//! Tests for the embedding `.Call` routines, each in its own R subprocess.

use super::*;
use crate::test_support::with_r_in_subprocess;
use serde_json::json;
use std::sync::Mutex;

/// Declare a test that runs `body` inside an isolated R process.
///
/// The first name is the test that is run normally; it launches the second,
/// ignored one in a child process where R is initialized.
macro_rules! r_test {
    ($outer:ident, $inner:ident, $body:expr) => {
        #[test]
        fn $outer() {
            $inner();
        }

        #[test]
        #[ignore = "runs in an isolated R subprocess via the test above"]
        fn $inner() {
            with_r_in_subprocess(concat!(module_path!(), "::", stringify!($inner)), $body);
        }
    };
}

/// Message the stub publisher rejects with. It contains conversion
/// specifications to prove that R prints it literally.
const REJECTION: &str = "rejected: 100% sure %s %d %%";

static RECEIVED: Mutex<Vec<(String, Value)>> = Mutex::new(Vec::new());

/// A publisher whose result is chosen by the topic.
fn stub_publisher(topic: &str, data: Value) -> PublishOutcome {
    RECEIVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((topic.to_owned(), data));
    match topic {
        "stub.none" => PublishOutcome::NoTarget,
        "stub.reject" => PublishOutcome::Rejected(REJECTION.to_owned()),
        "stub.panic" => panic!("stub publisher panic"),
        _ => PublishOutcome::Published,
    }
}

fn take_received() -> Vec<(String, Value)> {
    std::mem::take(
        &mut *RECEIVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

fn register_with_stub() {
    assert!(set_event_publisher(stub_publisher));
    crate::register_embedding_routines().expect("registration should succeed");
}

fn run(code: &str) {
    crate::eval_string(code).unwrap_or_else(|error| panic!("R code failed: {error:?}\n{code}"));
}

/// R helper shared by the tests below: the error message raised by `expr`.
const ERROR_HELPER: &str = r#"
publish_error <- function(...) {
  tryCatch(
    {
      .Call("arf_publish_event", ..., PACKAGE = "(embedding)")
      NULL
    },
    error = function(e) e
  )
}
"#;

r_test!(
    routines_are_unavailable_until_registered_and_report_the_api_version,
    routines_are_unavailable_until_registered_and_report_the_api_version_in_r,
    || {
        run(r#"
for (name in c("arf_api_version", "arf_publish_event")) {
  stopifnot(!is.loaded(name, PACKAGE = "(embedding)", type = ".Call"))
}
"#);
        crate::register_embedding_routines().expect("registration should succeed");
        run(r#"
for (name in c("arf_api_version", "arf_publish_event")) {
  stopifnot(is.loaded(name, PACKAGE = "(embedding)", type = ".Call"))
}
stopifnot(identical(.Call("arf_api_version", PACKAGE = "(embedding)"), 1L))
routines <- getDLLRegisteredRoutines("(embedding)")[[".Call"]]
stopifnot(identical(routines[["arf_api_version"]]$numParameters, 0L))
stopifnot(identical(routines[["arf_publish_event"]]$numParameters, 2L))
# R checks the arity, so a wrong call never reaches the routines.
stopifnot(inherits(
  tryCatch(.Call("arf_api_version", 1L, PACKAGE = "(embedding)"), error = identity),
  "error"
))
stopifnot(inherits(
  tryCatch(.Call("arf_publish_event", "a.b", PACKAGE = "(embedding)"), error = identity),
  "error"
))
stopifnot(inherits(
  tryCatch(.Call("arf_publish_event", "a.b", NULL, NULL, PACKAGE = "(embedding)"),
           error = identity),
  "error"
))
"#);
    }
);

r_test!(
    publishing_without_a_publisher_returns_false,
    publishing_without_a_publisher_returns_false_in_r,
    || {
        crate::register_embedding_routines().expect("registration should succeed");
        run(ERROR_HELPER);
        run(r#"
stopifnot(identical(
  .Call("arf_publish_event", "demo.topic", list(a = 1), PACKAGE = "(embedding)"),
  FALSE
))
# Bad arguments are errors whether or not anything could receive the event.
stopifnot(grepl("must be a single non-NA", conditionMessage(publish_error(NA_character_, NULL))))
stopifnot(grepl("cannot be converted", conditionMessage(publish_error("demo.topic", 1:2))))
"#);
    }
);

r_test!(
    the_publisher_receives_the_converted_event_and_decides_the_result,
    the_publisher_receives_the_converted_event_and_decides_the_result_in_r,
    || {
        register_with_stub();
        run(r#"
publish <- function(topic, data = NULL) .Call("arf_publish_event", topic, data, PACKAGE = "(embedding)")
stopifnot(identical(publish("stub.ok", list(a = 1.5, b = list("x", NULL, TRUE), n = 3L)), TRUE))
stopifnot(identical(publish("stub.ok"), TRUE))
stopifnot(identical(publish("stub.none", "text"), FALSE))
stopifnot(identical(publish(c(a = "stub.ok"), c(x = 1)), TRUE))
"#);
        assert_eq!(
            take_received(),
            vec![
                (
                    "stub.ok".to_owned(),
                    json!({"a": 1.5, "b": ["x", null, true], "n": 3})
                ),
                ("stub.ok".to_owned(), Value::Null),
                ("stub.none".to_owned(), json!("text")),
                ("stub.ok".to_owned(), json!(1.0)),
            ]
        );
    }
);

r_test!(
    invalid_arguments_raise_errors_without_reaching_the_publisher,
    invalid_arguments_raise_errors_without_reaching_the_publisher_in_r,
    || {
        register_with_stub();
        run(ERROR_HELPER);
        run(r#"
topic_message <- "`topic` must be a single non-NA character string"
for (topic in list(NA_character_, c("a.b", "c.d"), character(), 1, TRUE, NULL,
                   list("a.b"), factor("a.b"), as.name("a.b"))) {
  error <- publish_error(topic, NULL)
  stopifnot(inherits(error, "error"))
  stopifnot(identical(conditionMessage(error), topic_message))
  stopifnot(is.null(conditionCall(error)))
}

error <- publish_error("a.b", list(x = list(y = c(1, 2))))
stopifnot(inherits(error, "error"))
stopifnot(identical(
  conditionMessage(error),
  paste0("`data` cannot be converted to JSON: atomic vector of length 2 ",
         "cannot be converted; expected length 1 (at JSON Pointer \"/x/y\")")
))
stopifnot(grepl("NA cannot be converted", conditionMessage(publish_error("a.b", NA))))
stopifnot(grepl("class attribute", conditionMessage(publish_error("a.b", factor("x")))))
stopifnot(grepl("type closure", conditionMessage(publish_error("a.b", mean))))
"#);
        assert!(take_received().is_empty());
    }
);

r_test!(
    a_rejected_event_raises_its_message_literally,
    a_rejected_event_raises_its_message_literally_in_r,
    || {
        register_with_stub();
        run(ERROR_HELPER);
        run(&format!(
            r#"
rejection <- {REJECTION:?}
error <- publish_error("stub.reject", NULL)
stopifnot(inherits(error, "error"))
stopifnot(identical(conditionMessage(error), rejection))
stopifnot(is.null(conditionCall(error)))
"#
        ));
        assert_eq!(take_received().len(), 1);
    }
);

r_test!(
    r_keeps_working_after_many_errors,
    r_keeps_working_after_many_errors_in_r,
    || {
        register_with_stub();
        run(ERROR_HELPER);
        run(r#"
protected_before <- NULL
for (i in 1:3000) {
  stopifnot(inherits(publish_error("stub.reject", NULL), "error"))
  stopifnot(inherits(publish_error(NA_character_, NULL), "error"))
  stopifnot(inherits(publish_error("stub.ok", c(1, 2)), "error"))
}
# Garbage collection and allocation still work after the repeated unwinding.
invisible(gc())
x <- lapply(1:1000, function(i) as.character(i))
stopifnot(length(x) == 1000L)
stopifnot(identical(
  .Call("arf_publish_event", "stub.ok", list(i = 1L), PACKAGE = "(embedding)"),
  TRUE
))
# An error inside a function that is itself being evaluated is caught normally.
f <- function() .Call("arf_publish_event", "stub.reject", NULL, PACKAGE = "(embedding)")
stopifnot(identical(tryCatch(f(), error = function(e) "caught"), "caught"))
stopifnot(identical(try(f(), silent = TRUE) |> inherits("try-error"), TRUE))
"#);
    }
);

r_test!(
    a_panicking_publisher_becomes_an_r_error,
    a_panicking_publisher_becomes_an_r_error_in_r,
    || {
        register_with_stub();
        run(ERROR_HELPER);
        run(&format!(
            r#"
error <- publish_error("stub.panic", NULL)
stopifnot(inherits(error, "error"))
stopifnot(identical(conditionMessage(error), {PANIC_MESSAGE:?}))
stopifnot(identical(
  .Call("arf_publish_event", "stub.ok", NULL, PACKAGE = "(embedding)"),
  TRUE
))
"#
        ));
    }
);
