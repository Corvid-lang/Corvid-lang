//! Type system and effect checker.
//!
//! Walks a parsed, resolved `File` and validates type and effect rules.
//! The headline check is **approve-before-dangerous**: any call to a tool
//! declared `dangerous` must be preceded by a matching `approve` in the
//! same block, or compilation fails.
//!
//! See `ARCHITECTURE.md` §5–§6.

#![allow(dead_code)]

pub mod checker;
pub mod errors;
pub mod types;

pub use checker::{typecheck, Checked};
pub use errors::{TypeError, TypeErrorKind};
pub use types::Type;

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_resolve::resolve;
    use corvid_syntax::{lex, parse_file};

    fn check(src: &str) -> Checked {
        let tokens = lex(src).expect("lex failed");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse errors: {perr:?}");
        let resolved = resolve(&file);
        assert!(
            resolved.errors.is_empty(),
            "resolve errors: {:?}",
            resolved.errors
        );
        typecheck(&file, &resolved)
    }

    // =================================================================
    // Effect checks — the killer feature.
    // =================================================================

    #[test]
    fn safe_tool_without_approve_is_ok() {
        let src = "\
tool get_order(id: String) -> Order

type Order:
    id: String

agent fetch(id: String) -> Order:
    return get_order(id)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn dangerous_tool_without_approve_is_compile_error() {
        let src = "\
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

type Receipt:
    id: String

agent bad(id: String, amount: Float) -> Receipt:
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(
            !c.errors.is_empty(),
            "expected unapproved-dangerous error"
        );
        assert!(
            c.errors.iter().any(|e| matches!(
                e.kind,
                TypeErrorKind::UnapprovedDangerousCall { .. }
            )),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn dangerous_tool_with_matching_approve_is_ok() {
        let src = "\
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

type Receipt:
    id: String

agent ok(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn approve_label_wrong_case_still_works() {
        // snake_case comparison is case-tolerant via PascalCase roundtrip.
        let src = "\
tool send_email(to: String, body: String) -> Nothing dangerous

agent notify(to: String) -> Nothing:
    approve SendEmail(to, to)
    return send_email(to, to)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn approve_wrong_arity_does_not_authorize() {
        let src = "\
tool send_email(to: String, body: String) -> Nothing dangerous

agent notify(to: String) -> Nothing:
    approve SendEmail(to)
    return send_email(to, to)
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                e.kind,
                TypeErrorKind::UnapprovedDangerousCall { .. }
            )),
            "expected unapproved error for arity mismatch; got: {:?}",
            c.errors
        );
    }

    #[test]
    fn approve_does_not_leak_out_of_if_branch() {
        // The outer call must also have approval; the one inside the `if`
        // does not authorize the outer one.
        let src = "\
tool send_email(to: String, body: String) -> Nothing dangerous

agent notify(flag: Bool, to: String) -> Nothing:
    if flag:
        approve SendEmail(to, to)
        send_email(to, to)
    return send_email(to, to)
";
        let c = check(src);
        let unapproved_count = c
            .errors
            .iter()
            .filter(|e| matches!(e.kind, TypeErrorKind::UnapprovedDangerousCall { .. }))
            .count();
        assert_eq!(
            unapproved_count, 1,
            "expected exactly one unapproved-dangerous error (the outer call), got {:?}",
            c.errors
        );
    }

    #[test]
    fn outer_approve_authorizes_inner_call() {
        // An approve outside an `if` should authorize a call inside the `if`.
        let src = "\
tool send_email(to: String, body: String) -> Nothing dangerous

agent notify(flag: Bool, to: String) -> Nothing:
    approve SendEmail(to, to)
    if flag:
        send_email(to, to)
    return send_email(to, to)
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "outer approve should authorize inner call; got: {:?}",
            c.errors
        );
    }

    #[test]
    fn error_hint_suggests_the_approve_line() {
        let src = "\
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

type Receipt:
    id: String

agent bad(id: String, amount: Float) -> Receipt:
    return issue_refund(id, amount)
";
        let c = check(src);
        let err = c
            .errors
            .iter()
            .find(|e| matches!(e.kind, TypeErrorKind::UnapprovedDangerousCall { .. }))
            .expect("expected an unapproved-dangerous error");
        let hint = err.hint().expect("expected hint");
        assert!(hint.contains("approve"), "hint should mention approve: {hint}");
        assert!(
            hint.contains("IssueRefund"),
            "hint should include PascalCase label IssueRefund: {hint}"
        );
    }

    // =================================================================
    // Arity and type checks.
    // =================================================================

    #[test]
    fn arity_mismatch_is_flagged() {
        let src = "\
tool greet(name: String, title: String) -> String

agent call_wrong(n: String) -> String:
    return greet(n)
";
        let c = check(src);
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::ArityMismatch { .. })),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn unknown_field_is_flagged() {
        let src = "\
type Ticket:
    order_id: String

agent bad(t: Ticket) -> String:
    return t.nonexistent
";
        let c = check(src);
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::UnknownField { .. })),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn field_access_on_non_struct_is_flagged() {
        let src = "\
agent bad(x: String) -> String:
    return x.length
";
        let c = check(src);
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::NotAStruct { .. })),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn bare_function_reference_is_flagged() {
        // `get_order` without `()` is an error in v0.1.
        let src = "\
tool get_order(id: String) -> String

agent bad(id: String) -> String:
    f = get_order
    return f
";
        let c = check(src);
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::BareFunctionReference { .. })),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn string_plus_string_is_concatenation() {
        let src = "\
agent hello(name: String) -> String:
    return \"hello \" + name
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected String + String to typecheck, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn string_plus_int_still_errors() {
        let src = "\
agent bad(name: String) -> String:
    return name + 3
";
        let c = check(src);
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::TypeMismatch { .. })),
            "expected a TypeMismatch, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn type_as_value_is_flagged() {
        let src = "\
agent bad(x: String) -> String:
    y = String
    return y
";
        let c = check(src);
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::TypeAsValue { .. })),
            "got: {:?}",
            c.errors
        );
    }

    // =================================================================
    // Full canonical program
    // =================================================================

    #[test]
    fn refund_bot_typechecks_cleanly() {
        let src = r#"
import python "anthropic" as anthropic

type Ticket:
    order_id: String
    user_id: String

type Order:
    id: String
    amount: Float

type Decision:
    should_refund: Bool
    reason: String

type Receipt:
    refund_id: String
    amount: Float

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """Decide whether this ticket deserves a refund."""

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
"#;
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "canonical refund_bot should typecheck cleanly, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn refund_bot_without_approve_fails_to_compile() {
        // Identical to above but the approve line is gone.
        let src = r#"
type Ticket:
    order_id: String
    user_id: String

type Order:
    id: String
    amount: Float

type Decision:
    should_refund: Bool
    reason: String

type Receipt:
    refund_id: String
    amount: Float

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """Decide whether this ticket deserves a refund."""

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        issue_refund(order.id, order.amount)

    return decision
"#;
        let c = check(src);
        let unapproved: Vec<_> = c
            .errors
            .iter()
            .filter(|e| matches!(e.kind, TypeErrorKind::UnapprovedDangerousCall { .. }))
            .collect();
        assert_eq!(
            unapproved.len(),
            1,
            "exactly one unapproved-dangerous error expected. got: {:?}",
            c.errors
        );
        // The hint should tell the user exactly what to add.
        let hint = unapproved[0].hint().unwrap();
        assert!(hint.contains("approve IssueRefund"), "hint was: {hint}");
    }

    #[test]
    fn result_and_option_annotations_resolve_to_known_types() {
        let src = "\
tool fetch(id: String) -> Result<Option<String>, String>

agent load(id: String) -> Result<Option<String>, String>:
    return fetch(id)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn question_unwraps_result_in_matching_return_context() {
        let src = "\
tool fetch(id: String) -> Result<String, String>

agent load(id: String) -> Result<String, String>:
    value = fetch(id)?
    return Ok(value)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn question_unwraps_option_in_matching_return_context() {
        let src = "\
tool maybe_name(id: String) -> Option<String>

agent load(id: String) -> Option<String>:
    value = maybe_name(id)?
    return Some(value)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn question_on_non_result_option_errors_cleanly() {
        let src = "\
agent bad(x: String) -> String:
    return x?
";
        let c = check(src);
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::InvalidTryPropagate { .. })),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn question_with_mismatched_return_context_errors_cleanly() {
        let src = "\
tool fetch(id: String) -> Result<String, String>

agent bad(id: String) -> String:
    return fetch(id)?
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                e.kind,
                TypeErrorKind::TryPropagateReturnMismatch { .. }
            )),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn retry_expression_has_inner_type() {
        let src = "\
tool fetch_name(id: String) -> String

agent load(id: String) -> String:
    value = try fetch_name(id) on error retry 3 times backoff linear 25
    return value
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }
}
