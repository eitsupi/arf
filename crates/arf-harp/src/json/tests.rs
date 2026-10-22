//! Tests that do not need an embedded R.

use super::limits::{Budget, PathStack};
use super::to_r::validate;
use super::*;
use serde_json::{Value, json};

fn pointer(error: &JsonError) -> String {
    error.path().to_pointer()
}

fn validate_value(value: &Value) -> Result<(), JsonError> {
    validate(value, 0, &mut Budget::default(), &mut PathStack::default())
}

#[test]
fn pointer_escapes_slash_and_tilde() {
    // The two keys "a/b" and "a","b" must give different pointers.
    let nested = JsonPath::from_segments(vec![
        JsonPathSegment::Key("a".into()),
        JsonPathSegment::Key("b".into()),
    ]);
    let single = JsonPath::from_segments(vec![JsonPathSegment::Key("a/b".into())]);
    assert_eq!(nested.to_pointer(), "/a/b");
    assert_eq!(single.to_pointer(), "/a~1b");

    let tilde = JsonPath::from_segments(vec![JsonPathSegment::Key("c~d".into())]);
    assert_eq!(tilde.to_pointer(), "/c~0d");

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
fn error_display_mentions_reason_and_pointer() {
    let error = validate_value(&json!({"a/b": ["x", "y\u{0}"]})).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("NUL"), "{message}");
    assert!(message.contains("/a~1b/1"), "{message}");
}

#[test]
fn depth_limit_is_inclusive() {
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
fn depth_limit_applies_to_objects() {
    let mut value = json!(1);
    for _ in 0..=MAX_DEPTH {
        value = json!({ "k": value });
    }
    assert_eq!(
        validate_value(&value).unwrap_err().kind(),
        &JsonErrorKind::DepthExceeded { limit: MAX_DEPTH }
    );
}

#[test]
fn node_limit_counts_every_value() {
    let at_limit = Value::Array(vec![Value::Null; MAX_NODES - 1]);
    assert!(validate_value(&at_limit).is_ok());

    let over = Value::Array(vec![Value::Null; MAX_NODES]);
    let error = validate_value(&over).unwrap_err();
    assert_eq!(
        error.kind(),
        &JsonErrorKind::TooManyNodes { limit: MAX_NODES }
    );
    assert_eq!(pointer(&error), format!("/{}", MAX_NODES - 1));
}

#[test]
fn string_budget_covers_keys_and_values() {
    let mut budget = Budget::default();
    assert!(budget.count_string(MAX_STRING_BYTES - 1).is_ok());
    assert_eq!(budget.remaining_string_bytes(), 1);
    assert!(budget.count_string(1).is_ok());
    assert_eq!(
        budget.count_string(1),
        Err(JsonErrorKind::StringBytesExceeded {
            limit: MAX_STRING_BYTES
        })
    );
    assert!(Budget::default().count_string(usize::MAX).is_err());

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

    let mut object = serde_json::Map::new();
    object.insert(big, json!("v"));
    assert_eq!(
        validate_value(&Value::Object(object)).unwrap_err().kind(),
        &JsonErrorKind::StringBytesExceeded {
            limit: MAX_STRING_BYTES
        }
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

    // The same value, as `serde_json` produces it from escaped text.
    let value: Value = serde_json::from_str(r#"{"a":"x\u0000y"}"#).unwrap();
    assert_eq!(
        validate_value(&value).unwrap_err().kind(),
        &JsonErrorKind::NulInString
    );
}

#[test]
fn validate_checks_integer_range() {
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
}

#[test]
fn validate_accepts_any_finite_float() {
    // Not stored as an integer, so no integer range applies.
    assert!(validate_value(&json!(1.0e300)).is_ok());
    assert!(validate_value(&json!(9_007_199_254_740_993.0)).is_ok());
    assert!(validate_value(&json!(-0.0)).is_ok());
}

#[test]
fn validate_accepts_empty_containers_and_empty_keys() {
    assert!(validate_value(&json!({"": [], "a": {}})).is_ok());
}

#[test]
fn duplicate_keys_in_incoming_json_keep_the_last_value() {
    // `serde_json` resolves duplicates while parsing; the conversion sees the result.
    let value: Value = serde_json::from_str(r#"{"a":1,"a":2}"#).unwrap();
    assert_eq!(value, json!({"a": 2}));
    assert!(validate_value(&value).is_ok());
}
