//! Tests that run against an embedded R, each in its own subprocess.

use super::*;
use crate::ffi::{install_symbol, top_level_exec};
use crate::object::{RObject, eval_string_with_visibility};
use crate::test_support::with_r_in_subprocess;
use arf_libr::{SexpType, r_library};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Evaluate `code` and keep the result reachable from the global environment,
/// so that it stays alive however the protection stack is used afterwards.
fn r_value(code: &str) -> RObject {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let name = format!(
        ".arf_json_input_{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    eval_string_with_visibility(&format!("{name} <- {{ {code} }}"))
        .unwrap_or_else(|e| panic!("R code failed: {code}: {e}"));
    eval_string_with_visibility(&name)
        .unwrap_or_else(|e| panic!("R code failed: {code}: {e}"))
        .value
}

/// Bind `object` to `name` in the global environment.
fn bind(name: &str, object: &RObject) {
    let lib = r_library().unwrap();
    unsafe {
        let symbol = install_symbol(name).unwrap();
        (lib.rf_definevar)(symbol, object.sexp(), *lib.r_globalenv);
    }
}

/// Evaluate an R expression that must be `TRUE`.
fn r_true(code: &str) -> bool {
    let lib = r_library().unwrap();
    let result = eval_string_with_visibility(&format!("isTRUE(({code}\n))"))
        .unwrap_or_else(|e| panic!("R code failed: {code}: {e}"));
    unsafe { *(lib.logical)(result.value.sexp()) != 0 }
}

/// Convert the JSON `value` to R, bind it to `.jr`, and require `check` to hold there.
fn assert_json_to_r(value: Value, check: &str) {
    let object = json_to_r(&value).unwrap_or_else(|e| panic!("{value}: {e}"));
    bind(".jr", &object);
    assert!(r_true(check), "{value} did not satisfy: {check}");
}

/// Same, starting from JSON text parsed by `serde_json`.
fn assert_text_to_r(text: &str, check: &str) {
    let value: Value = serde_json::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let object = json_to_r(&value).unwrap_or_else(|e| panic!("{text}: {e}"));
    bind(".jr", &object);
    assert!(r_true(check), "{text} did not satisfy: {check}");
}

fn to_json(code: &str) -> Result<Value, JsonError> {
    r_to_json(&r_value(code))
}

fn assert_to_json(code: &str, expected: Value) {
    let actual = to_json(code).unwrap_or_else(|e| panic!("{code}: {e}"));
    assert_eq!(actual, expected, "{code}");
}

fn assert_to_json_err(code: &str, kind: JsonErrorKind, pointer: &str) {
    let error = to_json(code).expect_err(code);
    assert_eq!(error.kind(), &kind, "{code}: {error}");
    assert_eq!(error.path().to_pointer(), pointer, "{code}: {error}");
}

r_test!(
    json_scalars_become_length_one_vectors,
    json_scalars_become_length_one_vectors_in_r,
    || {
        assert_json_to_r(json!(true), "identical(.jr, TRUE)");
        assert_json_to_r(json!(false), "identical(.jr, FALSE)");
        assert_json_to_r(json!(1.5), "identical(.jr, 1.5)");
        assert_json_to_r(json!("x"), r#"identical(.jr, "x")"#);
        assert_json_to_r(json!(""), r#"identical(.jr, "")"#);
        assert_json_to_r(Value::Null, "is.null(.jr)");
        // Integers in the JSON text are still doubles in R.
        assert_json_to_r(json!(3), r#"identical(.jr, 3) && typeof(.jr) == "double""#);
        assert_json_to_r(json!(-7), "identical(.jr, -7)");
        assert_json_to_r(json!(9_007_199_254_740_992_i64), "identical(.jr, 2^53)");
        assert_json_to_r(json!(0.1), "identical(.jr, 0.1)");
        assert_json_to_r(json!(-0.0), "identical(.jr, -0) && 1 / .jr == -Inf");
        // Plain vectors: no attributes at all.
        assert_json_to_r(json!(1), "is.null(attributes(.jr)) && length(.jr) == 1L");
        assert_json_to_r(json!("x"), "is.null(attributes(.jr)) && length(.jr) == 1L");
    }
);

r_test!(
    json_arrays_become_unnamed_lists,
    json_arrays_become_unnamed_lists_in_r,
    || {
        assert_json_to_r(
            json!([]),
            "identical(.jr, list()) && is.null(attributes(.jr)) && is.null(names(.jr))",
        );
        assert_json_to_r(
            json!(["x"]),
            r#"identical(.jr, list("x")) && is.null(names(.jr)) && length(.jr) == 1L"#,
        );
        assert_json_to_r(
            json!([1]),
            "identical(.jr, list(1)) && is.null(attributes(.jr))",
        );
        assert_json_to_r(
            json!([null]),
            "identical(.jr, list(NULL)) && length(.jr) == 1L && is.null(.jr[[1]])",
        );
        assert_json_to_r(
            json!([null, 1, null]),
            "identical(.jr, list(NULL, 1, NULL)) && length(.jr) == 3L",
        );
        assert_json_to_r(
            json!([[], [[]], [1, [true]]]),
            "identical(.jr, list(list(), list(list()), list(1, list(TRUE))))",
        );
        assert_json_to_r(
            json!([{}, {"a": null}]),
            "identical(.jr, list(structure(list(), names = character(0)), list(a = NULL)))",
        );
    }
);

r_test!(
    json_objects_become_named_lists,
    json_objects_become_named_lists_in_r,
    || {
        assert_json_to_r(
            json!({"a": 1, "b": "x"}),
            r#"identical(.jr, list(a = 1, b = "x")) && identical(names(.jr), c("a", "b"))"#,
        );
        assert_json_to_r(
            json!({"a": null}),
            "identical(.jr, list(a = NULL)) && length(.jr) == 1L && identical(names(.jr), 'a')",
        );
        assert_json_to_r(
            json!({"a": {"b": [null, {}]}}),
            "identical(.jr, list(a = list(b = list(NULL, structure(list(), names = character(0))))))",
        );
        // A key that is the empty string is an ordinary key.
        assert_json_to_r(
            json!({"": 1}),
            r#"identical(.jr, structure(list(1), names = "")) && identical(names(.jr), "")"#,
        );
        assert_json_to_r(
            json!({"a/b": 1, "~": 2}),
            r#"setequal(names(.jr), c("a/b", "~")) && length(.jr) == 2L"#,
        );
    }
);

r_test!(
    empty_object_and_empty_array_differ,
    empty_object_and_empty_array_differ_in_r,
    || {
        assert_json_to_r(
            json!({}),
            r#"length(.jr) == 0L && is.list(.jr)
           && identical(attributes(.jr), list(names = character(0)))
           && !is.null(names(.jr))"#,
        );
        assert_json_to_r(
            json!([]),
            "length(.jr) == 0L && is.list(.jr) && is.null(attributes(.jr)) && is.null(names(.jr))",
        );
        assert_text_to_r(
            "{}",
            "identical(attributes(.jr), list(names = character(0)))",
        );
        assert_text_to_r("[]", "is.null(attributes(.jr))");
    }
);

r_test!(
    parsed_json_text_converts_to_r,
    parsed_json_text_converts_to_r_in_r,
    || {
        assert_text_to_r(
            r#" {"a": [1, "b", null, true], "c": {}, "d": [] } "#,
            r#"identical(.jr, list(a = list(1, "b", NULL, TRUE),
                             c = structure(list(), names = character(0)),
                             d = list()))"#,
        );
        assert_text_to_r("null", "is.null(.jr)");
        assert_text_to_r("  12.5 ", "identical(.jr, 12.5)");
        assert_text_to_r("9007199254740992", "identical(.jr, 2^53)");
        assert_text_to_r("-0", "identical(.jr, -0) && 1 / .jr == -Inf");
        assert_text_to_r("1e2", "identical(.jr, 100)");
    }
);

r_test!(
    json_rejections_surface_with_paths,
    json_rejections_surface_with_paths_in_r,
    || {
        let error = json_to_r(&json!([1, 9_007_199_254_740_993_i64])).unwrap_err();
        assert_eq!(error.kind(), &JsonErrorKind::IntegerOutOfRange);
        assert_eq!(error.path().to_pointer(), "/1");

        let error = json_to_r(&json!({"a/b": "x\u{0}y"})).unwrap_err();
        assert_eq!(error.kind(), &JsonErrorKind::NulInString);
        assert_eq!(error.path().to_pointer(), "/a~1b");

        // Every failure above leaves R usable.
        assert_text_to_r("[1]", "identical(.jr, list(1))");
    }
);

r_test!(
    duplicate_keys_in_incoming_json_keep_the_last,
    duplicate_keys_in_incoming_json_keep_the_last_in_r,
    || {
        // `serde_json` resolves the duplicate while parsing.
        assert_text_to_r(
            r#"{"a": 1, "b": [], "a": 2}"#,
            r#"identical(.jr[["a"]], 2) && length(.jr) == 2L"#,
        );
    }
);

r_test!(
    json_strings_are_marked_utf8,
    json_strings_are_marked_utf8_in_r,
    || {
        // The raw bytes are checked, so the result does not depend on the locale.
        assert_json_to_r(
            json!("\u{e9}"),
            r#"Encoding(.jr) == "UTF-8" && identical(charToRaw(.jr), as.raw(c(0xc3, 0xa9)))"#,
        );
        assert_json_to_r(
            json!("a\u{1f600}b"),
            r#"Encoding(.jr) == "UTF-8"
           && identical(charToRaw(.jr), as.raw(c(0x61, 0xf0, 0x9f, 0x98, 0x80, 0x62)))"#,
        );
        assert_json_to_r(
            json!({"k\u{e9}y": "\u{3042}"}),
            r#"Encoding(names(.jr)) == "UTF-8"
           && identical(charToRaw(names(.jr)), as.raw(c(0x6b, 0xc3, 0xa9, 0x79)))
           && identical(charToRaw(.jr[[1]]), as.raw(c(0xe3, 0x81, 0x82)))"#,
        );
        // ASCII stays unmarked, as R itself would have it.
        assert_json_to_r(json!("abc"), r#"Encoding(.jr) == "unknown""#);
        // Strings differing only in marking compare equal after translation.
        assert_json_to_r(
            json!("\u{e9}"),
            "identical(enc2utf8(.jr), intToUtf8(233L)) && .jr == intToUtf8(233L)",
        );
    }
);

r_test!(
    nul_is_rejected_and_conversion_recovers,
    nul_is_rejected_and_conversion_recovers_in_r,
    || {
        let error = json_to_r(&json!(["ok", {"k": "a\u{0}b"}])).unwrap_err();
        assert_eq!(error.kind(), &JsonErrorKind::NulInString);
        assert_eq!(error.path().to_pointer(), "/1/k");
        let error = json_to_r(&json!({"a\u{0}": 1})).unwrap_err();
        assert_eq!(error.kind(), &JsonErrorKind::NulInString);
        assert_json_to_r(json!(["fine"]), r#"identical(.jr, list("fine"))"#);
    }
);

r_test!(
    json_numbers_out_of_range_are_rejected,
    json_numbers_out_of_range_are_rejected_in_r,
    || {
        for number in [
            json!(9_007_199_254_740_993_i64),
            json!(-9_007_199_254_740_993_i64),
            json!(u64::MAX),
            json!(i64::MIN),
        ] {
            let error = json_to_r(&number).unwrap_err();
            assert_eq!(error.kind(), &JsonErrorKind::IntegerOutOfRange, "{number}");
            assert!(error.path().is_root());
        }
        assert_json_to_r(json!(-9_007_199_254_740_992_i64), "identical(.jr, -2^53)");
    }
);

r_test!(
    scalars_convert_to_json,
    scalars_convert_to_json_in_r,
    || {
        assert_to_json("NULL", Value::Null);
        assert_to_json("TRUE", json!(true));
        assert_to_json("FALSE", json!(false));
        assert_to_json("1.5", json!(1.5));
        assert_to_json("3", json!(3.0));
        assert_to_json("-0.25", json!(-0.25));
        assert_to_json("1e300", json!(1e300));
        assert_to_json("5L", json!(5));
        assert_to_json("-2147483647L", json!(-2_147_483_647_i64));
        assert_to_json(r#""x""#, json!("x"));
        assert_to_json(r#""""#, json!(""));
        // Doubles are doubles: no guessing that 3 might be an integer.
        let value = to_json("3").unwrap();
        assert!(value.as_i64().is_none() && value.as_f64() == Some(3.0));
    }
);

r_test!(lists_convert_to_json, lists_convert_to_json_in_r, || {
    assert_to_json("list()", json!([]));
    assert_to_json(r#"list("x")"#, json!(["x"]));
    assert_to_json("list(NULL)", json!([null]));
    assert_to_json("list(NULL, 1, NULL)", json!([null, 1.0, null]));
    assert_to_json("list(list(), list(list(1L)))", json!([[], [[1]]]));
    assert_to_json(
        r#"list(a = 1L, b = list(c = TRUE))"#,
        json!({"a": 1, "b": {"c": true}}),
    );
    assert_to_json("list(a = NULL)", json!({"a": null}));
    assert_to_json(
        "list(a = NULL, b = list(NULL, list(c = NULL)))",
        json!({"a": null, "b": [null, {"c": null}]}),
    );
    // A list with an empty names attribute is an object; one without is an array.
    assert_to_json("structure(list(), names = character(0))", json!({}));
    assert_to_json("setNames(list(), character(0))", json!({}));
    assert_to_json("list()", json!([]));
    assert_to_json(
        "list(a = list(), b = setNames(list(), character()))",
        json!({"a": [], "b": {}}),
    );
    // The empty string is a legitimate key.
    assert_to_json(r#"setNames(list(1L), "")"#, json!({"": 1}));
    assert_to_json(r#"list(a = 1L, 2L)"#, json!({"a": 1, "": 2}));
    assert_to_json(r#"setNames(list(1L), "a/b~c")"#, json!({"a/b~c": 1}));
});

r_test!(
    names_on_atomic_scalars_are_ignored,
    names_on_atomic_scalars_are_ignored_in_r,
    || {
        assert_to_json("c(a = 1L)", json!(1));
        assert_to_json(r#"c(k = "v")"#, json!("v"));
        assert_to_json("list(x = c(a = TRUE))", json!({"x": true}));
    }
);

r_test!(
    r_to_json_text_is_compact_json,
    r_to_json_text_is_compact_json_in_r,
    || {
        let text =
            r_to_json_text(&r_value(r#"list(a = 1L, b = "x", c = list(TRUE, NULL))"#)).unwrap();
        assert_eq!(text, r#"{"a":1,"b":"x","c":[true,null]}"#);
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"a": 1, "b": "x", "c": [true, null]})
        );
    }
);

r_test!(
    unrepresentable_values_are_rejected,
    unrepresentable_values_are_rejected_in_r,
    || {
        let unsupported = |name: &str| JsonErrorKind::UnsupportedType {
            type_name: name.to_string(),
        };
        let length = |n: usize| JsonErrorKind::InvalidLength { length: n };

        assert_to_json_err("c(1, 2)", length(2), "");
        assert_to_json_err("1:3", length(3), "");
        assert_to_json_err("numeric(0)", length(0), "");
        assert_to_json_err("character(0)", length(0), "");
        assert_to_json_err("logical(0)", length(0), "");
        assert_to_json_err("integer(0)", length(0), "");
        assert_to_json_err(r#"c("a", "b")"#, length(2), "");
        assert_to_json_err("c(TRUE, FALSE)", length(2), "");

        for code in ["NA", "NA_integer_", "NA_real_", "NA_character_"] {
            assert_to_json_err(code, JsonErrorKind::MissingValue, "");
        }
        assert_to_json_err("NaN", JsonErrorKind::NonFiniteNumber, "");
        assert_to_json_err("Inf", JsonErrorKind::NonFiniteNumber, "");
        assert_to_json_err("-Inf", JsonErrorKind::NonFiniteNumber, "");
        // NaN produced by arithmetic is not NA.
        assert_to_json_err("0 / 0", JsonErrorKind::NonFiniteNumber, "");

        assert_to_json_err("as.raw(1)", unsupported("raw"), "");
        assert_to_json_err("1i", unsupported("complex"), "");
        assert_to_json_err("globalenv()", unsupported("environment"), "");
        assert_to_json_err("function(x) x", unsupported("closure"), "");
        assert_to_json_err("sum", unsupported("builtin"), "");
        assert_to_json_err("quote(a)", unsupported("symbol"), "");
        assert_to_json_err("quote(a + b)", unsupported("language"), "");
        assert_to_json_err("expression(1)", unsupported("expression"), "");
        assert_to_json_err("pairlist(a = 1)", unsupported("pairlist"), "");
    }
);

r_test!(
    class_is_rejected_and_other_attributes_are_ignored,
    class_is_rejected_and_other_attributes_are_ignored_in_r,
    || {
        for code in [
            r#"factor("a")"#,
            "Sys.Date()",
            "data.frame(a = 1)",
            r#"structure(1, class = "x")"#,
            r#"structure(list(), class = "x")"#,
            r#"structure(list(a = 1), class = "x")"#,
            r#"structure(list(a = 1), class = "x", foo = 1)"#,
            "as.POSIXct(0, origin = '1970-01-01', tz = 'UTC')",
        ] {
            assert_to_json_err(code, JsonErrorKind::ClassAttribute, "");
        }
        // A class on a nested element is located.
        assert_to_json_err(
            r#"list(a = list(structure(1, class = "x")))"#,
            JsonErrorKind::ClassAttribute,
            "/a/0",
        );
        // Anything but class and names is not looked at.
        assert_to_json(r#"structure(1, foo = "x")"#, json!(1.0));
        assert_to_json("matrix(1)", json!(1.0));
        assert_to_json(r#"structure(1L, levels = "a")"#, json!(1));
        assert_to_json(r#"structure(list(), foo = 1)"#, json!([]));
        assert_to_json(
            r#"structure(list(a = 1), comment = "x", foo = 1)"#,
            json!({"a": 1.0}),
        );
        assert_to_json(
            "list(a = list(structure(1, tsp = c(1, 1, 1))))",
            json!({"a": [1.0]}),
        );
    }
);

r_test!(failures_are_located, failures_are_located_in_r, || {
    assert_to_json_err("list(1, NA)", JsonErrorKind::MissingValue, "/1");
    assert_to_json_err(
        "list(a = list(1, c(1, 2)))",
        JsonErrorKind::InvalidLength { length: 2 },
        "/a/1",
    );
    assert_to_json_err(
        r#"setNames(list(list(NA)), "a/b~c")"#,
        JsonErrorKind::MissingValue,
        "/a~1b~0c/0",
    );
    assert_to_json_err(
        r#"setNames(list(NaN), "")"#,
        JsonErrorKind::NonFiniteNumber,
        "/",
    );
    assert_to_json_err(
        "list(list(list(globalenv())))",
        JsonErrorKind::UnsupportedType {
            type_name: "environment".to_string(),
        },
        "/0/0/0",
    );
});

r_test!(bad_names_are_rejected, bad_names_are_rejected_in_r, || {
    assert_to_json_err("list(a = 1, a = 2)", JsonErrorKind::DuplicateKey, "/a");
    assert_to_json_err(
        r#"list(x = list(k = 1, j = 2, k = 3))"#,
        JsonErrorKind::DuplicateKey,
        "/x/k",
    );
    assert_to_json_err(
        r#"setNames(list(1, 2), c("", ""))"#,
        JsonErrorKind::DuplicateKey,
        "/",
    );
    assert_to_json_err(
        "setNames(list(1), NA_character_)",
        JsonErrorKind::NaName,
        "/0",
    );
    assert_to_json_err(
        r#"setNames(list(1, 2), c("a", NA))"#,
        JsonErrorKind::NaName,
        "/1",
    );
    assert_to_json_err(
        r#"list(inner = setNames(list(1), NA_character_))"#,
        JsonErrorKind::NaName,
        "/inner/0",
    );
});

r_test!(
    duplicates_are_judged_after_utf8_conversion,
    duplicates_are_judged_after_utf8_conversion_in_r,
    || {
        // The same text in two encodings is the same key.
        let code = r#"
        latin1 <- rawToChar(as.raw(c(0x63, 0x61, 0x66, 0xe9)))
        Encoding(latin1) <- "latin1"
        utf8 <- enc2utf8(latin1)
        stopifnot(!identical(charToRaw(latin1), charToRaw(utf8)))
        setNames(list(1, 2), c(latin1, utf8))
    "#;
        let error = to_json(code).unwrap_err();
        assert_eq!(error.kind(), &JsonErrorKind::DuplicateKey);
        assert_eq!(error.path().to_pointer(), "/caf\u{e9}");
    }
);

r_test!(
    strings_are_translated_to_utf8,
    strings_are_translated_to_utf8_in_r,
    || {
        let latin1 = r#"{
        x <- rawToChar(as.raw(c(0x63, 0x61, 0x66, 0xe9)))
        Encoding(x) <- "latin1"
        x
    }"#;
        assert_to_json(&format!("list({latin1})"), json!(["caf\u{e9}"]));
        assert_to_json(
            &format!("setNames(list(1L), {latin1})"),
            json!({"caf\u{e9}": 1}),
        );

        // Every byte of the upper Latin-1 range maps to the code point of the same value.
        let upper = to_json(
            r#"{
            x <- rawToChar(as.raw(0xa0:0xff))
            Encoding(x) <- "latin1"
            x
        }"#,
        )
        .unwrap();
        let expected: String = (0xa0u32..=0xff)
            .map(|c| char::from_u32(c).unwrap())
            .collect();
        assert_eq!(upper, Value::String(expected));

        // Strings already in UTF-8, including four-byte characters.
        assert_to_json(
            "intToUtf8(c(233L, 12354L, 128512L))",
            json!("\u{e9}\u{3042}\u{1f600}"),
        );
        assert_to_json(
            "setNames(list(1L), intToUtf8(12354L))",
            json!({"\u{3042}": 1}),
        );
        // A key containing a slash and a non-ASCII character appears escaped in the pointer.
        assert_to_json_err(
            "setNames(list(NA), paste0(intToUtf8(233L), '/x'))",
            JsonErrorKind::MissingValue,
            "/\u{e9}~1x",
        );
    }
);

r_test!(
    bytes_and_invalid_strings_are_rejected,
    bytes_and_invalid_strings_are_rejected_in_r,
    || {
        assert_to_json_err(
            r#"{
            x <- rawToChar(as.raw(c(0x61, 0xe9)))
            Encoding(x) <- "bytes"
            list(1, x)
        }"#,
            JsonErrorKind::BytesEncoding,
            "/1",
        );
        assert_to_json_err(
            r#"{
            x <- rawToChar(as.raw(c(0x61, 0xff, 0xfe)))
            Encoding(x) <- "UTF-8"
            list(a = x)
        }"#,
            JsonErrorKind::InvalidEncoding,
            "/a",
        );
        assert_to_json_err(
            r#"{
            x <- rawToChar(as.raw(c(0xc3)))
            Encoding(x) <- "UTF-8"
            setNames(list(1), x)
        }"#,
            JsonErrorKind::InvalidEncoding,
            "/0",
        );
        // The conversion machinery still works afterwards.
        assert_to_json("list(1L)", json!([1]));
    }
);

r_test!(
    depth_limit_applies_to_r_values,
    depth_limit_applies_to_r_values_in_r,
    || {
        let nested = |levels: usize| {
            format!(
                "{{ x <- list(); for (i in seq_len({})) x <- list(x); x }}",
                levels - 1
            )
        };
        assert!(to_json(&nested(MAX_DEPTH)).is_ok());
        let error = to_json(&nested(MAX_DEPTH + 1)).unwrap_err();
        assert_eq!(
            error.kind(),
            &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
        );
        assert_eq!(error.path().segments().len(), MAX_DEPTH);

        let object = |levels: usize| {
            format!(
                "{{ x <- list(); for (i in seq_len({})) x <- list(k = x); x }}",
                levels - 1
            )
        };
        assert!(to_json(&object(MAX_DEPTH)).is_ok());
        assert_eq!(
            to_json(&object(MAX_DEPTH + 1)).unwrap_err().kind(),
            &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
        );

        // The same limit holds going the other way.
        let mut value = json!([]);
        for _ in 1..MAX_DEPTH {
            value = Value::Array(vec![value]);
        }
        assert!(json_to_r(&value).is_ok());
        assert_eq!(
            json_to_r(&Value::Array(vec![value])).unwrap_err().kind(),
            &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
        );
    }
);

r_test!(
    node_limit_applies_to_r_values,
    node_limit_applies_to_r_values_in_r,
    || {
        let error = to_json(&format!("vector('list', {})", MAX_NODES)).unwrap_err();
        assert_eq!(
            error.kind(),
            &JsonErrorKind::TooManyNodes { limit: MAX_NODES }
        );
        assert_eq!(error.path().to_pointer(), format!("/{}", MAX_NODES - 1));
        assert_to_json("vector('list', 3L)", json!([null, null, null]));
    }
);

r_test!(
    round_trips_preserve_structure,
    round_trips_preserve_structure_in_r,
    || {
        let documents = [
            json!(null),
            json!(true),
            json!(1.5),
            json!("text"),
            json!([]),
            json!({}),
            json!([null]),
            json!({"a": null}),
            json!([[], {}, [null, [null]], {"a": {}}]),
            json!({"": "", "a": [1.5, "\u{e9}", false, null], "z": {"y": {"x": []}}}),
            json!({"a/b": {"c~d": [true]}}),
        ];
        for document in documents {
            let object = json_to_r(&document).unwrap();
            let back = r_to_json(&object).unwrap();
            assert_eq!(back, document);
            let text = r_to_json_text(&object).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), document);
        }
        // Whole numbers come back as numbers, if no longer integers.
        let object = json_to_r(&json!([1, 2, 3])).unwrap();
        assert_eq!(r_to_json(&object).unwrap(), json!([1.0, 2.0, 3.0]));
    }
);

r_test!(
    round_trips_from_r_values,
    round_trips_from_r_values_in_r,
    || {
        let original = r_value(
            r#"list(a = 1.5, b = list(), c = setNames(list(), character(0)), d = list(NULL, "x", TRUE))"#,
        );
        let value = r_to_json(&original).unwrap();
        let rebuilt = json_to_r(&value).unwrap();
        bind(".original", &original);
        bind(".rebuilt", &rebuilt);
        // Member order follows the JSON map, so compare by name.
        assert!(r_true(
            r#"{
            o <- .original[sort(names(.original))]
            r <- .rebuilt[sort(names(.rebuilt))]
            identical(o, r)
        }"#
        ));
    }
);

r_test!(
    conversions_survive_garbage_collection,
    conversions_survive_garbage_collection_in_r,
    || {
        let document = json!({
            "list": [1.5, "two", null, {"three": [true, {}], "": "empty key"}],
            "text": "caf\u{e9}",
            "object": {"a": {"b": {"c": []}}},
        });
        let from_r = r_value(
            r#"list(list = list(1.5, "two", NULL, setNames(list(list(TRUE, setNames(list(), character(0))), "empty key"), c("three", ""))),
                text = {
                    x <- rawToChar(as.raw(c(99L, 97L, 102L, 233L)))
                    Encoding(x) <- "latin1"
                    x
                },
                object = list(a = list(b = list(c = list()))))"#,
        );
        eval_string_with_visibility("gctorture(TRUE)").unwrap();
        let to_r = json_to_r(&document);
        let back = r_to_json(&from_r);
        eval_string_with_visibility("gctorture(FALSE)").unwrap();

        let to_r = to_r.unwrap();
        assert_eq!(back.unwrap(), document);
        assert_eq!(r_to_json(&to_r).unwrap(), document);
        // Results are still intact after more allocation.
        eval_string_with_visibility("invisible(gc())").unwrap();
        assert_eq!(r_to_json(&to_r).unwrap(), document);
    }
);

r_test!(
    results_nest_with_other_protections,
    results_nest_with_other_protections_in_r,
    || {
        // Hold several results at once and release them in reverse order, as scoping
        // does; each must stay valid until its own release.
        let first = json_to_r(&json!({"first": [1, 2, 3]})).unwrap();
        let second = json_to_r(&json!({"second": ["a", "b"]})).unwrap();
        {
            let third = json_to_r(&json!([null, {"third": {}}])).unwrap();
            eval_string_with_visibility("invisible(gc())").unwrap();
            assert_eq!(r_to_json(&third).unwrap(), json!([null, {"third": {}}]));
        }
        eval_string_with_visibility("invisible(gc())").unwrap();
        assert_eq!(r_to_json(&second).unwrap(), json!({"second": ["a", "b"]}));
        assert_eq!(
            r_to_json(&first).unwrap(),
            json!({"first": [1.0, 2.0, 3.0]})
        );
    }
);

r_test!(
    failures_do_not_disturb_later_conversions,
    failures_do_not_disturb_later_conversions_in_r,
    || {
        let before = json_to_r(&json!(["before"])).unwrap();
        for _ in 0..50 {
            assert!(to_json("list(1, list(2, c(1, 2)))").is_err());
            assert!(to_json("list(a = 1, a = 2)").is_err());
            assert!(to_json(r#"list(function() 1)"#).is_err());
            assert!(json_to_r(&json!(["a", "b\u{0}"])).is_err());
        }
        let after = json_to_r(&json!({"after": [true]})).unwrap();
        eval_string_with_visibility("invisible(gc())").unwrap();
        assert_eq!(r_to_json(&before).unwrap(), json!(["before"]));
        assert_eq!(r_to_json(&after).unwrap(), json!({"after": [true]}));
        assert_eq!(
            to_json("list(x = list(1L, NULL))").unwrap(),
            json!({"x": [1, null]})
        );
    }
);

r_test!(
    an_r_error_in_a_protected_call_is_reported,
    an_r_error_in_a_protected_call_is_reported_in_r,
    || {
        let lib = r_library().unwrap();
        // Allocating a vector of negative length raises an R error.
        let failed = unsafe {
            top_level_exec(move || (lib.rf_allocvector)(SexpType::IntSxp as i32, -1) as usize)
        };
        assert!(failed.is_err());

        // The same call with a valid length succeeds, and R is intact afterwards.
        let ok = unsafe {
            top_level_exec(move || (lib.rf_allocvector)(SexpType::IntSxp as i32, 1) as usize)
        };
        assert!(ok.is_ok());
        assert_to_json("list(1L)", json!([1]));
    }
);

r_test!(
    oversized_strings_are_rejected_before_copying,
    oversized_strings_are_rejected_before_copying_in_r,
    || {
        let limit = JsonErrorKind::StringBytesExceeded {
            limit: MAX_STRING_BYTES,
        };
        assert_to_json_err(
            &format!("list(strrep('a', {}))", MAX_STRING_BYTES + 1),
            limit.clone(),
            "/0",
        );
        // The budget is shared by all strings and names in the document.
        assert_to_json_err(
            &format!("list(strrep('a', {}), 'b', 'c')", MAX_STRING_BYTES - 1),
            limit.clone(),
            "/2",
        );
        assert_to_json_err(
            &format!("setNames(list(1L), strrep('k', {}))", MAX_STRING_BYTES + 1),
            limit,
            "/0",
        );
        assert_to_json("list('ok')", json!(["ok"]));
    }
);
