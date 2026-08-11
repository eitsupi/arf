//! Syntactic policy for code received through IPC.
//!
//! This is deliberately a best-effort check.  It is not an R sandbox and it
//! cannot guarantee that an allowed call is non-mutating.

use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

use tree_sitter::Node;

fn is_inert_kind(kind: &str) -> bool {
    matches!(kind, "comment" | "comma")
}

#[derive(Debug, Clone, Default)]
pub struct EvalPolicy {
    pub allowlist: HashSet<String>,
    pub unrestricted: bool,
}

static POLICY: OnceLock<RwLock<EvalPolicy>> = OnceLock::new();

pub fn set_policy(targets: impl IntoIterator<Item = String>, unrestricted: bool) {
    let policy = EvalPolicy {
        allowlist: targets.into_iter().collect(),
        unrestricted,
    };
    let lock = POLICY.get_or_init(|| RwLock::new(EvalPolicy::default()));
    *lock.write().unwrap_or_else(|e| e.into_inner()) = policy;
}

pub fn policy() -> EvalPolicy {
    POLICY
        .get_or_init(|| RwLock::new(EvalPolicy::default()))
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Validate an IPC evaluation before it reaches R.
pub fn validate(code: &str) -> Result<(), String> {
    let policy = policy();
    validate_with_policy(code, &policy)
}

fn validate_with_policy(code: &str, policy: &EvalPolicy) -> Result<(), String> {
    let tree =
        crate::r_parser::parse_r(code).ok_or_else(|| "R code could not be parsed".to_string())?;
    let root = tree.root_node();
    if root.has_error() || contains_missing(&root) {
        return Err("R code contains a syntax error or missing token".to_string());
    }
    if policy.unrestricted {
        return Ok(());
    }
    let source = code.as_bytes();
    let mut cursor = root.walk();
    let mut saw_expression = false;
    for child in root
        .named_children(&mut cursor)
        .filter(|child| child.kind() != "comment")
    {
        saw_expression = true;
        validate_top_level(child, source, &policy.allowlist)?;
    }
    if !saw_expression {
        return Err("empty IPC evaluation is not allowed".to_string());
    }
    Ok(())
}

fn contains_missing(node: &Node<'_>) -> bool {
    if node.is_missing() {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| contains_missing(&child))
}

fn validate_top_level(
    node: Node<'_>,
    source: &[u8],
    allowlist: &HashSet<String>,
) -> Result<(), String> {
    match node.kind() {
        "comment" => Ok(()),
        "call" | "binary_operator" | "unary_operator" | "subset" | "subset2"
        | "extract_operator" => validate_expression(node, source, allowlist),
        "identifier" | "string" | "integer" | "float" | "complex" | "true" | "false" | "null"
        | "inf" | "nan" | "na" => Ok(()),
        "parenthesized_expression" => {
            let mut cursor = node.walk();
            let mut children = node
                .named_children(&mut cursor)
                .filter(|child| child.kind() != "comment");
            match (children.next(), children.next()) {
                (Some(child), None) => validate_top_level(child, source, allowlist),
                _ => Err("parenthesized IPC expression is not a single operation".to_string()),
            }
        }
        kind => Err(format!(
            "R construct '{kind}' is not allowed at the top level of IPC evaluation"
        )),
    }
}

fn validate_call(node: Node<'_>, source: &[u8], allowlist: &HashSet<String>) -> Result<(), String> {
    let callee = node
        .named_child(0)
        .ok_or_else(|| "call has no function target".to_string())?;
    let target = call_target(callee, source).ok_or_else(|| {
        "computed, special-form, and ::: call targets are not allowed".to_string()
    })?;
    if !allowlist.contains(&target) {
        return Err(format!(
            "IPC evaluation target '{target}' is not allowlisted"
        ));
    }

    // Walk every argument and nested call. Literals are execution-inert, but
    // identifiers are syntactic leaves whose evaluation can force a promise or
    // run an active binding. That is acceptable because IPC eval cannot create
    // such bindings; assignment is never permitted. Keep the accepted leaves
    // explicit so newly introduced grammar nodes fail closed.
    let mut cursor = node.walk();
    for child in node
        .named_children(&mut cursor)
        .skip(1)
        .filter(|child| !is_inert_kind(child.kind()))
    {
        validate_expression(child, source, allowlist)?;
    }
    Ok(())
}

fn call_target(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => {
            let text = node.utf8_text(source).ok()?;
            (!text.contains('`')).then(|| text.to_string())
        }
        "namespace" | "namespace_operator" => {
            let text = node.utf8_text(source).ok()?;
            if text.contains(":::") {
                None
            } else {
                let mut cursor = node.walk();
                let named: Vec<_> = node.named_children(&mut cursor).collect();
                if named.len() == 2
                    && named.iter().all(|n| n.kind() == "identifier")
                    && text.matches("::").count() == 1
                {
                    let lhs = named[0].utf8_text(source).ok()?;
                    let rhs = named[1].utf8_text(source).ok()?;
                    (!lhs.contains('`') && !rhs.contains('`')).then(|| format!("{lhs}::{rhs}"))
                } else {
                    None
                }
            }
        }
        _ => None,
    }
}

fn validate_expression(
    node: Node<'_>,
    source: &[u8],
    allowlist: &HashSet<String>,
) -> Result<(), String> {
    match node.kind() {
        "comment" => Ok(()),
        "call" => validate_call(node, source, allowlist),
        "binary_operator" => validate_binary_operator(node, source, allowlist),
        "unary_operator" => validate_unary_operator(node, source, allowlist),
        "subset" => validate_index_operator(node, source, allowlist, "["),
        "subset2" => validate_index_operator(node, source, allowlist, "[["),
        "extract_operator" => validate_extract_operator(node, source, allowlist),
        "identifier" | "string" | "integer" | "float" | "complex" | "true" | "false" | "null"
        | "inf" | "nan" | "na" | "dots" | "dot_dot_i" => Ok(()),
        "arguments" | "argument" | "named_argument" | "parenthesized_expression" => {
            let mut cursor = node.walk();
            for child in node
                .named_children(&mut cursor)
                .filter(|child| !is_inert_kind(child.kind()))
            {
                validate_expression(child, source, allowlist)?;
            }
            Ok(())
        }
        // These nodes include assignment, function definitions, control flow,
        // namespace lookup outside a call target, and other special forms.
        kind => Err(format!("R construct '{kind}' is not allowed by IPC policy")),
    }
}

fn require_target(target: &str, allowlist: &HashSet<String>) -> Result<(), String> {
    if allowlist.contains(target) {
        Ok(())
    } else {
        Err(format!(
            "IPC evaluation target '{target}' is not allowlisted"
        ))
    }
}

fn field<'tree>(node: Node<'tree>, name: &str) -> Result<Node<'tree>, String> {
    node.child_by_field_name(name)
        .ok_or_else(|| format!("{} has no {name}", node.kind()))
}

fn operator_target(node: Node<'_>, source: &[u8]) -> Result<String, String> {
    field(node, "operator")?
        .utf8_text(source)
        .map(str::to_string)
        .map_err(|_| format!("{} contains invalid UTF-8", node.kind()))
}

fn validate_binary_operator(
    node: Node<'_>,
    source: &[u8],
    allowlist: &HashSet<String>,
) -> Result<(), String> {
    let target = operator_target(node, source)?;
    if matches!(target.as_str(), "<-" | "<<-" | "->" | "->>" | "=" | ":=") {
        return Err(format!("assignment operator '{target}' is never allowed"));
    }
    // Native pipes have call-rewriting semantics rather than behaving like an
    // ordinary function target, so keep them outside the allowlist model.
    if target == "|>" {
        return Err("native pipe operator '|>' is never allowed".to_string());
    }
    require_target(&target, allowlist)?;
    validate_expression(field(node, "lhs")?, source, allowlist)?;
    validate_expression(field(node, "rhs")?, source, allowlist)
}

fn validate_unary_operator(
    node: Node<'_>,
    source: &[u8],
    allowlist: &HashSet<String>,
) -> Result<(), String> {
    let target = operator_target(node, source)?;
    require_target(&target, allowlist)?;
    validate_expression(field(node, "rhs")?, source, allowlist)
}

fn validate_index_operator(
    node: Node<'_>,
    source: &[u8],
    allowlist: &HashSet<String>,
    target: &str,
) -> Result<(), String> {
    require_target(target, allowlist)?;
    validate_expression(field(node, "function")?, source, allowlist)?;
    validate_expression(field(node, "arguments")?, source, allowlist)
}

fn validate_extract_operator(
    node: Node<'_>,
    source: &[u8],
    allowlist: &HashSet<String>,
) -> Result<(), String> {
    let target = operator_target(node, source)?;
    require_target(&target, allowlist)?;
    validate_expression(field(node, "lhs")?, source, allowlist)?;
    if let Some(rhs) = node.child_by_field_name("rhs") {
        validate_expression(rhs, source, allowlist)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(code: &str, targets: &[&str]) -> Result<(), String> {
        let policy = EvalPolicy {
            allowlist: targets.iter().map(|s| (*s).to_string()).collect(),
            unrestricted: false,
        };
        validate_with_policy(code, &policy)
    }

    #[test]
    fn allows_nested_allowlisted_calls_and_literals() {
        assert!(check("outer(inner(1))", &["outer", "inner"]).is_ok());
    }

    #[test]
    fn allows_multiple_arguments() {
        // The grammar exposes argument separators as named `comma` nodes, so
        // the traversal has to skip them the same way it skips comments.
        assert!(check("mean(1, 2)", &["mean"]).is_ok());
        assert!(check("outer(1, inner(2), 3)", &["outer", "inner"]).is_ok());
    }

    #[test]
    fn allows_comments_in_calls_and_parenthesized_expressions() {
        assert!(
            check(
                r#"mean(1, # note
                2)"#,
                &["mean"]
            )
            .is_ok()
        );
        assert!(
            check(
                r#"(# note
                1 + 1)"#,
                &["+"]
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_unknown_nested_call_and_mutation() {
        assert!(check("outer(system('x'))", &["outer"]).is_err());
        assert!(check("x <- outer(1)", &["outer"]).is_err());
        assert!(check(":::('x')", &["x"]).is_err());
    }

    #[test]
    fn allows_exact_namespace_target_only() {
        assert!(check("stats::median(1)", &["stats::median"]).is_ok());
        assert!(check("stats:::median(1)", &["stats:::median"]).is_err());
        assert!(check("stats::median(1)", &["median"]).is_err());
    }

    #[test]
    fn allows_exact_operator_targets() {
        assert!(check("1 + length(x)", &["+", "length"]).is_ok());
        assert!(check("-1", &["-"]).is_ok());
        assert!(check("x[1]", &["["]).is_ok());
        assert!(check("x[[1]]", &["[["]).is_ok());
        assert!(check("x$name", &["$"]).is_ok());
    }

    #[test]
    fn rejects_unlisted_or_structurally_unsafe_operators() {
        assert!(check("1 + 2", &[]).is_err());
        assert!(check(r#"1"#, &["+"]).is_ok());
        assert!(check(r#"x"#, &["+"]).is_ok());
        assert!(check("x <- 1", &["<-"]).is_err());
        assert!(check("x |> length()", &["|>", "length"]).is_err());
    }

    #[test]
    fn allows_bare_literals_and_identifiers_without_allowlist() {
        assert!(check(r#"1"#, &[]).is_ok());
        assert!(check(r#"x"#, &[]).is_ok());
    }

    #[test]
    fn extraction_requires_an_allowlisted_operator() {
        assert!(check(r#"x$a"#, &[]).is_err());
        assert!(check(r#"x$a"#, &["$"]).is_ok());
    }

    #[test]
    fn permanently_banned_constructs_stay_rejected_with_allowlist_entries() {
        let allowlist = ["<-", "if", "pkg:::fun", "|>", "length"];
        assert!(check(r#"x <- 1"#, &allowlist).is_err());
        assert!(check(r#"if (x) 1 else 2"#, &allowlist).is_err());
        assert!(check(r#"pkg:::fun(1)"#, &allowlist).is_err());
        assert!(check(r#"x |> length()"#, &allowlist).is_err());
    }

    #[test]
    fn unrestricted_still_requires_parse_success() {
        let policy = EvalPolicy {
            allowlist: HashSet::new(),
            unrestricted: true,
        };
        assert!(validate_with_policy("x <- 1", &policy).is_ok());
        assert!(validate_with_policy("x <-", &policy).is_err());
    }
}
