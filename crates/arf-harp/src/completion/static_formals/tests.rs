use super::lookup::inspection_error_outcome;
use super::parse::{is_valid_identifier, named_argument_name, parse_request};
use super::{
    StaticFormal, StaticFormalsMode, StaticFormalsOutcome, StaticFormalsPolicy,
    StaticFormalsResult, candidates, lookup_with_policy, production_candidates,
};
use rd_rds::package::DefaultPresence;
use rd_rds::package::InstalledCodeError;
use rd_rds::package::StoredKind;

#[test]
fn parses_only_initial_supported_shape() {
    assert_eq!(
        parse_request("stats::lm(", 10),
        Some(super::StaticFormalsRequest {
            package: "stats".to_owned(),
            exported_name: "lm".to_owned(),
            partial: "".to_owned(),
            used_named: Vec::new(),
        })
    );
    assert_eq!(
        parse_request("stats::lm(fo", "stats::lm(fo".len()),
        Some(super::StaticFormalsRequest {
            package: "stats".to_owned(),
            exported_name: "lm".to_owned(),
            partial: "fo".to_owned(),
            used_named: Vec::new(),
        })
    );
    assert!(parse_request("stats:::lm(", "stats:::lm(".len()).is_none());
    assert!(parse_request("stats::lm(foo(", "stats::lm(foo(".len()).is_none());
    assert!(parse_request("stats::.foo(", "stats::.foo(".len()).is_some());
    assert!(parse_request("stats::foo_1(", "stats::foo_1(".len()).is_some());
    assert!(parse_request("stats::if(", "stats::if(".len()).is_none());
    assert!(parse_request("stats::NA_real_(", "stats::NA_real_(".len()).is_none());
    assert!(parse_request("stats::...(", "stats::...(".len()).is_none());
    assert!(parse_request("stats::..1(", "stats::..1(".len()).is_none());
    assert!(parse_request("stats::-foo(", "stats::-foo(".len()).is_none());
    assert!(parse_request("stats::_foo(", "stats::_foo(".len()).is_none());
    assert!(parse_request("stats::.1foo(", "stats::.1foo(".len()).is_none());
    assert!(parse_request("stats::.(", "stats::.(".len()).is_some());
    assert!(parse_request("stats::é(", "stats::é(".len()).is_none());
    assert_eq!(
        parse_request("stats::lm(foo = 1, ", "stats::lm(foo = 1, ".len()),
        Some(super::StaticFormalsRequest {
            package: "stats".to_owned(),
            exported_name: "lm".to_owned(),
            partial: "".to_owned(),
            used_named: vec!["foo".to_owned()],
        })
    );
    assert_eq!(
        parse_request("stats::lm(foo = 1, fo", "stats::lm(foo = 1, fo".len()),
        Some(super::StaticFormalsRequest {
            package: "stats".to_owned(),
            exported_name: "lm".to_owned(),
            partial: "fo".to_owned(),
            used_named: vec!["foo".to_owned()],
        })
    );
    assert_eq!(
        parse_request("stats::lm(foo = 1, bar", "stats::lm(foo = 1, bar".len())
            .expect("partial argument should parse")
            .used_named,
        vec!["foo".to_owned()]
    );
    let comparison = parse_request("stats::lm(x == 1, ", "stats::lm(x == 1, ".len())
        .expect("comparison expression should retain static completion");
    assert!(comparison.used_named.is_empty());
    let comparison_result = StaticFormalsResult {
        package: "stats".to_owned(),
        exported_name: "lm".to_owned(),
        source_name: Some("lm".to_owned()),
        kind: None,
        partial: comparison.partial,
        used_named: comparison.used_named,
        outcome: StaticFormalsOutcome::Available(vec![StaticFormal {
            name: "x".to_owned(),
            default: DefaultPresence::Absent,
        }]),
    };
    assert_eq!(candidates(&comparison_result), Some(vec!["x".to_owned()]));
    assert!(parse_request("stats::lm(foo = 1", "stats::lm(foo = 1".len()).is_none());
    assert!(parse_request("stats::lm(foo +", "stats::lm(foo +".len()).is_none());
    assert!(parse_request("stats::lm", "stats::lm".len()).is_none());
    assert!(parse_request("../stats::lm(", "../stats::lm(".len()).is_none());
    assert!(parse_request("stats::lm(foo = f(1), ", "stats::lm(foo = f(1), ".len()).is_none());
    assert!(parse_request("stats::lm(foo = \"x\", ", "stats::lm(foo = \"x\", ".len()).is_none());
    assert!(
        parse_request(
            "stats::lm(foo = 1 # comment",
            "stats::lm(foo = 1 # comment".len()
        )
        .is_none()
    );
}

#[test]
fn rejects_brackets_and_braces_in_arguments() {
    for line in [
        "stats::lm(x = value[fo",
        "stats::lm(x = value[1, fo",
        "stats::lm(x = value[1, ",
        "stats::lm(x = value[[1, fo",
        "stats::lm(x = { value[1, fo",
        "stats::lm(x = value[1], fo",
        "stats::lm(x = {1; 2}, fo",
        "stats::lm(x = value], fo",
        "stats::lm(x = value}, fo",
    ] {
        assert!(parse_request(line, line.len()).is_none(), "{line}");
    }

    let line = "stats::lm(x = value[1, fo], data = df)";
    let cursor = line.find("fo]").unwrap() + 2;
    assert!(parse_request(line, cursor).is_none());
}

#[test]
fn candidates_filter_partial_used_names_and_duplicates() {
    let result = StaticFormalsResult {
        package: "stats".to_owned(),
        exported_name: "lm".to_owned(),
        source_name: Some("lm".to_owned()),
        kind: None,
        partial: "fo".to_owned(),
        used_named: vec!["formula".to_owned()],
        outcome: StaticFormalsOutcome::Available(vec![
            StaticFormal {
                name: "formula".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "foo".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "foo".to_owned(),
                default: DefaultPresence::Absent,
            },
        ]),
    };
    assert_eq!(candidates(&result), Some(vec!["foo".to_owned()]));
    assert_eq!(
        candidates(&StaticFormalsResult {
            outcome: StaticFormalsOutcome::Unavailable("unsafe".to_owned()),
            ..result
        }),
        None
    );
}

#[test]
fn syntactic_identifier_validation_is_conservative() {
    for identifier in ["foo", ".foo", "foo_1", "."] {
        assert!(is_valid_identifier(identifier), "{identifier}");
    }
    for identifier in [
        "",
        "-foo",
        "_foo",
        ".1foo",
        "éfoo",
        "if",
        "TRUE",
        "NA_character_",
        "...",
        "..1",
        "..20",
    ] {
        assert!(!is_valid_identifier(identifier), "{identifier}");
    }
}

#[test]
fn static_policy_uses_exact_public_names_and_package_precedence() {
    let policy = StaticFormalsPolicy {
        mode: StaticFormalsMode::PreferStatic,
        excluded_packages: vec!["S7".to_owned()],
        excluded_functions: vec!["rlang::abort".to_owned()],
    };
    assert!(policy.excludes("S7", "anything"));
    assert!(policy.excludes("rlang", "abort"));
    assert!(!policy.excludes("rlang", "Abort"));
    assert!(!policy.excludes("s7", "anything"));
}

#[test]
fn static_policy_off_does_not_lookup() {
    let policy = StaticFormalsPolicy::off();
    let line = "missing_package::function(";
    assert!(lookup_with_policy(line, line.len(), &policy).is_none());
}

#[test]
fn production_candidates_match_r_formatting() {
    let result = StaticFormalsResult {
        package: "pkg".to_owned(),
        exported_name: "fun".to_owned(),
        source_name: Some("fun".to_owned()),
        kind: Some(StoredKind::Closure),
        partial: String::new(),
        used_named: Vec::new(),
        outcome: StaticFormalsOutcome::Available(vec![
            StaticFormal {
                name: "formula".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "...".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: ".foo".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "foo_1".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: ".".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "-foo".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "_foo".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: ".1foo".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "if".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "éfoo".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: String::new(),
                default: DefaultPresence::Absent,
            },
        ]),
    };
    assert_eq!(production_candidates(&result), None);
    let valid_result = StaticFormalsResult {
        outcome: StaticFormalsOutcome::Available(vec![
            StaticFormal {
                name: "formula".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "...".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: ".foo".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: "foo_1".to_owned(),
                default: DefaultPresence::Absent,
            },
            StaticFormal {
                name: ".".to_owned(),
                default: DefaultPresence::Absent,
            },
        ]),
        ..result.clone()
    };
    assert_eq!(
        production_candidates(&valid_result),
        Some(vec![
            "formula=".to_owned(),
            "...".to_owned(),
            ".foo=".to_owned(),
            "foo_1=".to_owned(),
            ".=".to_owned()
        ])
    );
    assert_eq!(
        production_candidates(&StaticFormalsResult {
            outcome: StaticFormalsOutcome::Available(Vec::new()),
            ..valid_result.clone()
        }),
        None
    );
    assert_eq!(
        production_candidates(&StaticFormalsResult {
            partial: "does_not_match".to_owned(),
            ..valid_result.clone()
        }),
        None
    );
    assert_eq!(
        production_candidates(&StaticFormalsResult {
            kind: Some(StoredKind::BuiltIn),
            ..valid_result.clone()
        }),
        None
    );
    assert_eq!(
        production_candidates(&StaticFormalsResult {
            kind: None,
            ..valid_result.clone()
        }),
        None
    );
    assert_eq!(
        production_candidates(&StaticFormalsResult {
            outcome: StaticFormalsOutcome::Available(vec![StaticFormal {
                name: "not-a-syntactic-name".to_owned(),
                default: DefaultPresence::Absent,
            }]),
            ..valid_result
        }),
        None
    );
}

#[test]
fn unsafe_binding_resolution_errors_fall_back_to_unresolved() {
    for error in [
        InstalledCodeError::UnknownStoredBinding {
            name: "missing".to_owned(),
        },
        InstalledCodeError::AmbiguousStoredBinding {
            name: "duplicate".to_owned(),
            count: 2,
        },
    ] {
        assert!(matches!(
            inspection_error_outcome("name", error),
            StaticFormalsOutcome::Unresolved(_)
        ));
    }
}

#[test]
fn other_binding_errors_are_unavailable() {
    let error = InstalledCodeError::InvalidPackageDirectory {
        path: std::path::PathBuf::from("pkg"),
    };
    assert!(matches!(
        inspection_error_outcome("name", error),
        StaticFormalsOutcome::Unavailable(_)
    ));
}

#[test]
fn comparisons_are_not_named_argument_separators() {
    for argument in ["x == 1", "x != 1", "x <= 1", "x >= 1"] {
        assert_eq!(named_argument_name(argument), None, "{argument}");
    }
    assert_eq!(named_argument_name("foo = 1"), Some("foo".to_owned()));
    assert_eq!(named_argument_name("foo=1"), Some("foo".to_owned()));
}
