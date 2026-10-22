//! Tests that do not need an embedded R.

use super::limits::{Budget, PathStack};
use super::to_r::validate;
use super::*;
use serde_json::{Value, json};

fn parse_err(text: &str) -> JsonError {
    parse_json_text(text).expect_err("text should be rejected")
}

fn pointer(error: &JsonError) -> String {
    error.path().to_pointer()
}

fn validate_value(value: &Value) -> Result<(), JsonError> {
    validate(value, 0, &mut Budget::default(), &mut PathStack::default())
}

fn nested_arrays(levels: usize) -> String {
    format!("{}{}", "[".repeat(levels), "]".repeat(levels))
}

#[test]
fn parses_scalars_and_containers() {
    assert_eq!(parse_json_text("null").unwrap(), Value::Null);
    assert_eq!(parse_json_text(" true ").unwrap(), json!(true));
    assert_eq!(parse_json_text("false").unwrap(), json!(false));
    assert_eq!(parse_json_text("[]").unwrap(), json!([]));
    assert_eq!(parse_json_text("{}").unwrap(), json!({}));
    assert_eq!(
        parse_json_text(r#" { "a" : [ 1 , null , "x" ] , "b" : { } } "#).unwrap(),
        json!({"a": [1, null, "x"], "b": {}})
    );
}

#[test]
fn parses_string_escapes() {
    assert_eq!(
        parse_json_text(r#""a\"b\\c\/d\b\f\n\r\t""#).unwrap(),
        json!(format!(r#"a"b\c/d{}"#, "\u{8}\u{c}\n\r\t"))
    );
    assert_eq!(
        parse_json_text(r#""\u00e9\u3042""#).unwrap(),
        json!("\u{e9}\u{3042}")
    );
    assert_eq!(
        parse_json_text(r#""\ud83d\ude00""#).unwrap(),
        json!("\u{1f600}")
    );
    // Characters written directly, not escaped.
    assert_eq!(
        parse_json_text(r#""é😀""#).unwrap(),
        json!("\u{e9}\u{1f600}")
    );
}

#[test]
fn empty_key_is_allowed() {
    assert_eq!(parse_json_text(r#"{"":1}"#).unwrap(), json!({"": 1}));
}

#[test]
fn rejects_lone_surrogates_and_bad_escapes() {
    for text in [
        r#""\ud83d""#,
        r#""\ud83dx""#,
        r#""\ud83d\u0041""#,
        r#""\ude00""#,
        r#""\x""#,
        r#""\u12""#,
        r#""\u12g4""#,
    ] {
        assert!(
            matches!(parse_err(text).kind(), JsonErrorKind::Syntax { .. }),
            "{text}"
        );
    }
}

#[test]
fn rejects_malformed_text() {
    for text in [
        "",
        "   ",
        "[1,]",
        "[,1]",
        r#"{"a":1,}"#,
        r#"{"a"}"#,
        "{a:1}",
        "{'a':1}",
        "[1 2]",
        "[1] 2",
        "nul",
        "nulll",
        "True",
        r#""abc"#,
        "[",
        r#"{"a":"#,
        "\u{feff}null",
        "NaN",
        "Infinity",
        "// c\nnull",
    ] {
        assert!(
            matches!(parse_err(text).kind(), JsonErrorKind::Syntax { .. }),
            "{text:?}"
        );
    }
}

#[test]
fn rejects_raw_control_characters_in_strings() {
    for control in ['\n', '\t', '\r', '\u{1}', '\u{1f}'] {
        let text = format!(r#""a{control}b""#);
        assert!(
            matches!(parse_err(&text).kind(), JsonErrorKind::Syntax { .. }),
            "{control:?}"
        );
    }
    // DEL and non-ASCII characters are ordinary.
    assert!(parse_json_text(&format!(r#""a{}é""#, '\u{7f}')).is_ok());
}

#[test]
fn rejects_malformed_numbers() {
    for text in [
        "01", "-", "+1", ".5", "1.", "1e", "1e+", "--1", "0x10", "1_0", "-01",
    ] {
        assert!(
            matches!(parse_err(text).kind(), JsonErrorKind::Syntax { .. }),
            "{text}"
        );
    }
}

#[test]
fn number_boundaries() {
    assert_eq!(
        parse_json_text("9007199254740992").unwrap().as_i64(),
        Some(9_007_199_254_740_992)
    );
    assert_eq!(
        parse_json_text("-9007199254740992").unwrap().as_i64(),
        Some(-9_007_199_254_740_992)
    );
    for text in [
        "9007199254740993",
        "-9007199254740993",
        "12345678901234567890",
        "123456789012345678901234567890",
        "-99999999999999999999999999999999999999999",
    ] {
        assert_eq!(
            parse_err(text).kind(),
            &JsonErrorKind::IntegerOutOfRange,
            "{text}"
        );
    }
}

#[test]
fn floats_are_not_range_checked_as_integers() {
    // Not written as an integer, so it is an ordinary double.
    assert_eq!(
        parse_json_text("9007199254740993.0").unwrap().as_f64(),
        Some(9007199254740992.0)
    );
    assert_eq!(parse_json_text("1e300").unwrap().as_f64(), Some(1e300));
    assert_eq!(parse_json_text("1E2").unwrap().as_f64(), Some(100.0));
    assert_eq!(parse_json_text("-1.5e-3").unwrap().as_f64(), Some(-0.0015));
    assert_eq!(parse_json_text("0.1").unwrap().as_f64(), Some(0.1));
}

#[test]
fn negative_zero_keeps_its_sign() {
    let value = parse_json_text("-0").unwrap();
    let number = value.as_f64().unwrap();
    assert_eq!(number, 0.0);
    assert!(number.is_sign_negative());
}

#[test]
fn rejects_non_finite_literals() {
    assert_eq!(parse_err("1e999").kind(), &JsonErrorKind::NonFiniteNumber);
    assert_eq!(parse_err("-1e999").kind(), &JsonErrorKind::NonFiniteNumber);
    assert_eq!(
        parse_err("[0, 1.5e400]").kind(),
        &JsonErrorKind::NonFiniteNumber
    );
    assert_eq!(pointer(&parse_err("[0, 1.5e400]")), "/1");
}

#[test]
fn rejects_duplicate_keys() {
    let error = parse_err(r#"{"a":1,"a":2}"#);
    assert_eq!(error.kind(), &JsonErrorKind::DuplicateKey);
    assert_eq!(pointer(&error), "/a");
}

#[test]
fn duplicate_keys_are_compared_after_unescaping() {
    let error = parse_err(r#"{"a":1,"\u0061":2}"#);
    assert_eq!(error.kind(), &JsonErrorKind::DuplicateKey);
    assert_eq!(pointer(&error), "/a");

    let error = parse_err(r#"{"\ud83d\ude00":1,"😀":2}"#);
    assert_eq!(error.kind(), &JsonErrorKind::DuplicateKey);
}

#[test]
fn duplicate_keys_are_found_at_any_depth() {
    let error = parse_err(r#"{"x":[0,{"k":{"a":1,"b":2,"a":3}}]}"#);
    assert_eq!(error.kind(), &JsonErrorKind::DuplicateKey);
    assert_eq!(pointer(&error), "/x/1/k/a");
}

#[test]
fn same_key_in_different_objects_is_fine() {
    assert_eq!(
        parse_json_text(r#"[{"a":1},{"a":2},{"a":{"a":3}}]"#).unwrap(),
        json!([{"a": 1}, {"a": 2}, {"a": {"a": 3}}])
    );
}

#[test]
fn duplicate_empty_keys_are_rejected() {
    let error = parse_err(r#"{"":1,"":2}"#);
    assert_eq!(error.kind(), &JsonErrorKind::DuplicateKey);
    assert_eq!(pointer(&error), "/");
}

#[test]
fn pointer_escapes_slash_and_tilde() {
    let error = parse_err(r#"{"a/b":{"c~d":{"e":1,"e":2}}}"#);
    assert_eq!(pointer(&error), "/a~1b/c~0d/e");

    // The two keys "a/b" and "a","b" must give different pointers.
    let nested = JsonPath::from_segments(vec![
        JsonPathSegment::Key("a".into()),
        JsonPathSegment::Key("b".into()),
    ]);
    let single = JsonPath::from_segments(vec![JsonPathSegment::Key("a/b".into())]);
    assert_eq!(nested.to_pointer(), "/a/b");
    assert_eq!(single.to_pointer(), "/a~1b");
    assert_ne!(nested.to_pointer(), single.to_pointer());

    // "~1" typed literally in a key is not confused with an escaped slash.
    let literal = JsonPath::from_segments(vec![JsonPathSegment::Key("~1".into())]);
    assert_eq!(literal.to_pointer(), "/~01");
}

#[test]
fn pointer_of_root_and_indices() {
    assert_eq!(JsonPath::root().to_pointer(), "");
    assert!(JsonPath::root().is_root());
    let path = JsonPath::from_segments(vec![
        JsonPathSegment::Key("items".into()),
        JsonPathSegment::Index(12),
        JsonPathSegment::Key(String::new()),
    ]);
    assert_eq!(path.to_pointer(), "/items/12/");
}

#[test]
fn syntax_error_reports_where_it_happened() {
    let error = parse_err(r#"{"a":[1,2,x]}"#);
    assert!(matches!(error.kind(), JsonErrorKind::Syntax { .. }));
    assert_eq!(pointer(&error), "/a/2");
}

#[test]
fn error_display_mentions_reason_and_pointer() {
    let message = parse_err(r#"{"a/b":{"k":1,"k":2}}"#).to_string();
    assert!(message.contains("duplicate"), "{message}");
    assert!(message.contains("/a~1b/k"), "{message}");
}

#[test]
fn depth_limit_is_inclusive() {
    let ok = nested_arrays(MAX_DEPTH);
    assert!(parse_json_text(&ok).is_ok());

    let error = parse_err(&nested_arrays(MAX_DEPTH + 1));
    assert_eq!(
        error.kind(),
        &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
    );
}

#[test]
fn depth_limit_applies_to_objects_and_mixtures() {
    let mut text = String::new();
    for level in 0..=MAX_DEPTH {
        text.push_str(if level % 2 == 0 { r#"{"k":"# } else { "[" });
    }
    text.push('1');
    for level in (0..=MAX_DEPTH).rev() {
        text.push(if level % 2 == 0 { '}' } else { ']' });
    }
    assert_eq!(
        parse_err(&text).kind(),
        &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
    );
}

#[test]
fn absurd_nesting_is_rejected_without_exhausting_the_stack() {
    let text = "[".repeat(1_000_000);
    assert_eq!(
        parse_err(&text).kind(),
        &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
    );
}

#[test]
fn node_limit_counts_every_value() {
    let at_limit = format!("[{}]", vec!["0"; MAX_NODES - 1].join(","));
    assert!(parse_json_text(&at_limit).is_ok());

    let over = format!("[{}]", vec!["0"; MAX_NODES].join(","));
    assert_eq!(
        parse_err(&over).kind(),
        &JsonErrorKind::TooManyNodes { limit: MAX_NODES }
    );
}

#[test]
fn text_size_limit() {
    let text = format!(r#""{}""#, "a".repeat(MAX_TEXT_BYTES));
    assert!(matches!(
        parse_err(&text).kind(),
        JsonErrorKind::TextTooLarge { limit, .. } if *limit == MAX_TEXT_BYTES
    ));
}

#[test]
fn string_budget_covers_keys_and_values() {
    let mut budget = Budget::default();
    assert!(budget.count_string(MAX_STRING_BYTES - 1).is_ok());
    assert!(budget.count_string(1).is_ok());
    assert_eq!(
        budget.count_string(1),
        Err(JsonErrorKind::StringBytesExceeded {
            limit: MAX_STRING_BYTES
        })
    );

    let mut budget = Budget::default();
    assert!(budget.count_string(usize::MAX).is_err());

    let big = "a".repeat(MAX_STRING_BYTES);
    assert!(validate_value(&json!([big])).is_ok());
    let error = validate_value(&json!([big.clone(), "b"])).unwrap_err();
    assert_eq!(
        error.kind(),
        &JsonErrorKind::StringBytesExceeded {
            limit: MAX_STRING_BYTES
        }
    );
    assert_eq!(pointer(&error), "/1");
}

#[test]
fn string_budget_counts_keys() {
    let key = "k".repeat(MAX_STRING_BYTES);
    let mut object = serde_json::Map::new();
    object.insert(key, json!("v"));
    let error = validate_value(&Value::Object(object)).unwrap_err();
    assert_eq!(
        error.kind(),
        &JsonErrorKind::StringBytesExceeded {
            limit: MAX_STRING_BYTES
        }
    );
}

#[test]
fn node_budget_for_values() {
    let mut budget = Budget::default();
    for _ in 0..MAX_NODES {
        budget.count_node().unwrap();
    }
    assert_eq!(
        budget.count_node(),
        Err(JsonErrorKind::TooManyNodes { limit: MAX_NODES })
    );
}

#[test]
fn validate_applies_the_depth_limit_to_values() {
    let mut value = json!(1);
    for _ in 0..MAX_DEPTH {
        value = Value::Array(vec![value]);
    }
    assert!(validate_value(&value).is_ok());
    let error = validate_value(&Value::Array(vec![value])).unwrap_err();
    assert_eq!(
        error.kind(),
        &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
    );
}

#[test]
fn validate_rejects_nul_in_strings_and_keys() {
    let error = validate_value(&json!({"a": ["x", "y\u{0}z"]})).unwrap_err();
    assert_eq!(error.kind(), &JsonErrorKind::NulInString);
    assert_eq!(pointer(&error), "/a/1");

    let error = validate_value(&json!({"k\u{0}": 1})).unwrap_err();
    assert_eq!(error.kind(), &JsonErrorKind::NulInString);
    assert_eq!(pointer(&error), "/k\u{0}");

    // The same document arrives through text with an escape.
    let value = parse_json_text(r#"{"a":"x\u0000y"}"#).unwrap();
    assert_eq!(
        validate_value(&value).unwrap_err().kind(),
        &JsonErrorKind::NulInString
    );
}

#[test]
fn validate_checks_integer_range_of_values() {
    assert!(validate_value(&json!(9_007_199_254_740_992_i64)).is_ok());
    assert!(validate_value(&json!(-9_007_199_254_740_992_i64)).is_ok());
    for number in [
        json!(9_007_199_254_740_993_i64),
        json!(-9_007_199_254_740_993_i64),
        json!(i64::MIN),
        json!(i64::MAX),
        json!(u64::MAX),
    ] {
        assert_eq!(
            validate_value(&number).unwrap_err().kind(),
            &JsonErrorKind::IntegerOutOfRange,
            "{number}"
        );
    }
    // Floats are not integers, however large.
    assert!(validate_value(&json!(1.0e300)).is_ok());
    assert!(validate_value(&json!(9_007_199_254_740_993.0)).is_ok());
}

#[test]
fn validate_accepts_empty_containers_and_empty_keys() {
    assert!(validate_value(&json!({"": [], "a": {}})).is_ok());
}
