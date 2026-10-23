//! Real-R tests for published handler lifetime and invocation.

use super::*;
use crate::{eval_string, test_support::with_r_in_subprocess};
use serde_json::json;

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

r_test!(
    expose_validates_the_whole_list_before_publishing,
    expose_validates_the_whole_list_before_publishing_in_r,
    || {
        let mut registry = HandlerRegistry::new().expect("create registry");
        for expression in [
            "list()",
            "structure(list(function(x) x, function(x) x), names = c('demo/one', 'demo/one'))",
            "list('demo/one' = function(x) x, 'demo/two' = 1)",
            "list('arf/private' = function(x) x)",
            "list('missing-slash' = function(x) x)",
            "list('demo/' = function(x) x)",
            "list('demo/broken' = function(x) {",
        ] {
            assert!(
                registry.expose(expression).is_err(),
                "accepted {expression}"
            );
            assert!(!registry.is_locked());
            assert!(registry.names().is_empty());
        }

        assert_eq!(
            registry
                .expose("structure(list(), names = character())")
                .unwrap(),
            Vec::<String>::new()
        );
        assert!(registry.is_locked());
        assert!(registry.expose("list('demo/again' = identity)").is_err());

        let mut registry = HandlerRegistry::new().expect("create second registry");
        arf_libr::start_ipc_capture(false);
        assert_eq!(
            registry.expose("1; list('demo/id' = identity)").unwrap(),
            vec!["demo/id".to_owned()]
        );
        let (stdout, _) = arf_libr::finish_ipc_capture();
        assert!(
            stdout.is_empty(),
            "visible intermediate value was printed: {stdout:?}"
        );
    }
);

r_test!(
    calls_retain_functions_convert_values_and_capture_output,
    calls_retain_functions_convert_values_and_capture_output_in_r,
    || {
        let mut registry = HandlerRegistry::new().expect("create registry");
        let names = registry
            .expose(r#"
local({
  hidden <- list(answer = 42L)
  list(
    "demo/echo" = function(x) { cat("out:", x$n, "\n"); message("warning-like"); x },
    "demo/get" = function(x) hidden$answer,
    "demo/fail" = function(x) { cat("before-error\n"); stop(structure(list(message = "handler failed"), class = c("demo_error", "error", "condition"))) },
    "demo/interrupt" = function(x) stop(structure(list(message = "interrupted"), class = c("interrupt", "condition"))),
    "demo/empty-interrupt" = function(x) stop(structure(list(), class = c("interrupt", "condition"))),
    "demo/broken-condition" = local({
      conditionMessage.demo_error <- function(e) stop("formatter failed")
      function(x) stop(structure(list(message = "original"), class = c("demo_error", "error", "condition")))
    }),
    "demo/bad-return" = function(x) function() NULL
  )
})
"#)
            .expect("publish handlers");
        assert_eq!(names.len(), 7);

        let response = registry
            .call("demo/echo", &json!({"n": 7}))
            .expect("call echo handler");
        assert_eq!(response.value, json!({"n": 7.0}));
        assert!(response.stdout.contains("out: 7"), "{:?}", response.stdout);
        assert!(
            response.stderr.contains("warning-like"),
            "{:?}",
            response.stderr
        );

        let retained = registry.call("demo/get", &Value::Null).unwrap();
        assert_eq!(retained.value, json!(42));

        let error = registry.call("demo/fail", &Value::Null).unwrap_err();
        assert_eq!(error.kind, HandlerErrorKind::RError);
        assert_eq!(error.message, "handler failed");
        assert!(error.class.iter().any(|name| name == "demo_error"));
        assert!(error.stdout.contains("before-error"));

        let error = registry.call("demo/interrupt", &Value::Null).unwrap_err();
        assert_eq!(error.kind, HandlerErrorKind::Interrupted);
        assert_eq!(error.message, "interrupted");

        let error = registry
            .call("demo/empty-interrupt", &Value::Null)
            .unwrap_err();
        assert_eq!(error.kind, HandlerErrorKind::Interrupted);
        assert_eq!(error.message, "Interrupted");

        let error = registry
            .call("demo/broken-condition", &Value::Null)
            .unwrap_err();
        assert_eq!(error.kind, HandlerErrorKind::RError);

        let error = registry.call("demo/bad-return", &Value::Null).unwrap_err();
        assert_eq!(error.kind, HandlerErrorKind::ReturnConversion);

        eval_string("gc()\ngc()").expect("collect after exposure");
        assert_eq!(
            registry.call("demo/get", &Value::Null).unwrap().value,
            json!(42)
        );
    }
);

r_test!(
    calls_distinguish_argument_errors_missing_names_and_revoke,
    calls_distinguish_argument_errors_missing_names_and_revoke_in_r,
    || {
        let mut registry = HandlerRegistry::new().expect("create registry");
        registry
            .expose("list('demo/id' = identity)")
            .expect("publish handler");

        assert_eq!(
            registry.call("demo/absent", &Value::Null).unwrap_err().kind,
            HandlerErrorKind::NotFound
        );
        assert_eq!(
            registry
                .call("demo/id", &json!(1u64 << 54))
                .unwrap_err()
                .kind,
            HandlerErrorKind::ArgumentConversion
        );
        assert_eq!(
            registry
                .call("demo/id", &json!({"ok": true}))
                .unwrap()
                .value,
            json!({"ok": true})
        );

        assert_eq!(registry.revoke(), vec!["demo/id".to_owned()]);
        assert!(registry.is_revoked());
        assert!(!registry.is_locked());
        assert_eq!(
            registry.call("demo/id", &json!(1)).unwrap_err().kind,
            HandlerErrorKind::NotFound
        );
        assert!(registry.expose("list('demo/new' = identity)").is_err());
    }
);

r_test!(
    warnings_follow_r_warn_option_and_do_not_defer,
    warnings_follow_r_warn_option_and_do_not_defer_in_r,
    || {
        let mut registry = HandlerRegistry::new().expect("create registry");
        registry
            .expose("list('demo/warn' = function(x) { warning('deferred-warning'); x }, 'demo/handled-warn' = function(x) withCallingHandlers({ warning('handled-warning'); x }, warning = function(w) invokeRestart('muffleWarning')))")
            .expect("publish warning handler");

        eval_string("options(warn = 0)").expect("set default warning behavior");
        let response = registry
            .call("demo/warn", &json!("ok"))
            .expect("call warning handler");
        assert_eq!(response.value, json!("ok"));
        assert!(
            response.stderr.contains("Warning: deferred-warning"),
            "{:?}",
            response.stderr
        );
        let response = registry
            .call("demo/handled-warn", &json!("ok"))
            .expect("call handler with its own warning handler");
        assert!(response.stderr.is_empty(), "{:?}", response.stderr);
        arf_libr::start_ipc_capture(false);
        eval_string("invisible(1)").expect("run next top-level expression");
        let (_, later_stderr) = arf_libr::finish_ipc_capture();
        assert!(
            !later_stderr.contains("deferred-warning"),
            "warning escaped after the handler call: {later_stderr:?}"
        );

        eval_string("options(warn = 2)").expect("set warning-as-error behavior");
        let error = registry.call("demo/warn", &json!("ok")).unwrap_err();
        assert_eq!(error.kind, HandlerErrorKind::RError);

        eval_string("options(warn = -1)").expect("set ignored warning behavior");
        let response = registry
            .call("demo/warn", &json!("ok"))
            .expect("call ignored warning handler");
        assert_eq!(response.value, json!("ok"));
        assert!(response.stderr.is_empty(), "{:?}", response.stderr);
    }
);

r_test!(
    retained_handlers_survive_gctorture,
    retained_handlers_survive_gctorture_in_r,
    || {
        eval_string("gctorture(TRUE)").expect("enable gctorture");
        let mut registry = HandlerRegistry::new().expect("create registry under gctorture");
        registry
            .expose("list('demo/gc' = function(x) list(ok = x))")
            .expect("publish under gctorture");
        let response = registry
            .call("demo/gc", &json!("alive"))
            .expect("call under gctorture");
        assert_eq!(response.value, json!({"ok": "alive"}));
        registry.revoke();
        drop(registry);
        eval_string("gctorture(FALSE)").expect("disable gctorture");

        // Repeated create/expose/drop cycles catch protection-stack imbalance
        // that might not fail in a single registry lifetime.
        for _ in 0..32 {
            let mut registry = HandlerRegistry::new().expect("create temporary registry");
            registry
                .expose("list('demo/cycle' = identity)")
                .expect("publish temporary handler");
        }
    }
);
