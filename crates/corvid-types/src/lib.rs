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
pub mod effects;
pub mod errors;
pub mod repl;
pub mod types;

pub use checker::{typecheck, Checked};
pub use errors::{TypeError, TypeErrorKind};
pub use repl::{CheckedTurn, ReplLocal, ReplSession, ReplTurnBuild, REPL_RESULT_NAME};
pub use effects::{
    analyze_effects, check_grounded_returns, AgentEffectSummary, ComposedProfile,
    ConstraintViolation, EffectProfile, EffectRegistry, ProvenanceViolation,
};
pub use types::Type;

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_resolve::{resolve, ResolveErrorKind};
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

    fn resolve_errors(src: &str) -> Vec<corvid_resolve::ResolveError> {
        let tokens = lex(src).expect("lex failed");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse errors: {perr:?}");
        resolve(&file).errors
    }

    fn mutate_once(base: &str, from: &str, to: &str) -> String {
        assert!(base.contains(from), "mutation source missing `{from}`");
        base.replacen(from, to, 1)
    }

    fn has_effect_violation(c: &Checked, dimension: &str) -> bool {
        c.errors.iter().any(|e| matches!(
            &e.kind,
            TypeErrorKind::EffectConstraintViolation { dimension: d, .. } if d == dimension
        ))
    }

    const MUTATION_APPROVAL_BASE: &str = r#"
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund(flag: Bool, id: String, amount: Float) -> Receipt:
    if flag:
        approve IssueRefund(id, amount)
        return issue_refund(id, amount)
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
"#;

    const MUTATION_EFFECT_BASE: &str = r#"
effect transfer_money:
    cost: $0.50
    reversible: false
    trust: human_required
    data: financial

effect audit_log:
    cost: $0.25
    trust: supervisor_required

type Ticket:
    order_id: String

type Order:
    id: String
    amount: Float

type Decision:
    should_refund: Bool

type Receipt:
    id: String

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses transfer_money
tool log_refund(id: String) -> Nothing uses audit_log

prompt decide(ticket: Ticket, order: Order) -> Decision:
    "Decide."

@budget($2.00)
@trust(autonomous)
agent safe_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide(ticket, order)
    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)
    return decision
"#;

    const MUTATION_PROVENANCE_BASE: &str = r#"
effect retrieval:
    data: grounded

type Ticket:
    order_id: String

type Order:
    id: String

type Decision:
    verdict: Bool

tool get_order(id: String) -> Grounded<Order> uses retrieval

prompt decide(ticket: Ticket, order: Order) -> Grounded<Decision>:
    "Decide."

agent grounded_bot(ticket: Ticket) -> Grounded<Decision>:
    order = get_order(ticket.order_id)
    decision = decide(ticket, order)
    return decision
"#;

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
        assert!(
            c.errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::InvalidRetryTarget { .. })),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn retry_expression_accepts_result_and_option_bodies() {
        let src = "\
tool fetch_name(id: String) -> Result<String, String>
tool maybe_name(id: String) -> Option<String>

agent load_result(id: String) -> Result<String, String>:
    return try fetch_name(id) on error retry 3 times backoff linear 25

agent load_option(id: String) -> Option<String>:
    return try maybe_name(id) on error retry 3 times backoff exponential 10
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn weak_new_is_fresh_immediately_on_construction() {
        let src = "\
agent make(name: String) -> Weak<String, {tool_call}>:
    return Weak::new(name)

agent load(name: String) -> Option<String>:
    return Weak::upgrade(make(name))
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn weak_upgrade_after_invalidating_effect_is_rejected() {
        let src = "\
tool fetch_name(id: String) -> String

agent make(name: String) -> Weak<String, {tool_call}>:
    return Weak::new(name)

agent load(name: String) -> Option<String>:
    w = make(name)
    fetch_name(name)
    return Weak::upgrade(w)
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                e.kind,
                TypeErrorKind::WeakUpgradeAcrossEffects { .. }
            )),
            "got: {:?}",
            c.errors
        );
    }

    #[test]
    fn weak_upgrade_is_allowed_after_refreshing_with_new() {
        let src = "\
tool fetch_name(id: String) -> String

agent make(name: String) -> Weak<String, {tool_call}>:
    return Weak::new(name)

agent load(name: String) -> Option<String>:
    w = make(name)
    fetch_name(name)
    w = make(name)
    return Weak::upgrade(w)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn weak_refresh_merges_by_all_paths_not_any_path() {
        let src = "\
tool fetch_name(id: String) -> String

agent make(name: String) -> Weak<String, {tool_call}>:
    return Weak::new(name)

agent load(flag: Bool, name: String) -> Option<String>:
    w = make(name)
    if flag:
        Weak::upgrade(w)
    else:
        keep = name
    fetch_name(name)
    return Weak::upgrade(w)
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                e.kind,
                TypeErrorKind::WeakUpgradeAcrossEffects { .. }
            )),
            "expected merge to require refresh on every predecessor; got {:?}",
            c.errors
        );
    }

    #[test]
    fn weak_type_rejects_non_heap_targets() {
        let src = "\
agent bad(x: Int) -> Weak<Int, {tool_call}>:
    return Weak::new(x)
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                e.kind,
                TypeErrorKind::InvalidWeakTargetType { .. }
                    | TypeErrorKind::InvalidWeakNewTarget { .. }
            )),
            "got: {:?}",
            c.errors
        );
    }

    // =================================================================
    // Mutation suite — dimensional effects, provenance, approval.
    // =================================================================

    #[test]
    fn mutation_remove_approve_line_errors() {
        // Removing the approve line must be caught — this is the core safety invariant.
        let src = mutate_once(
            MUTATION_APPROVAL_BASE,
            "        approve IssueRefund(id, amount)\n",
            "",
        );
        let c = check(&src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UnapprovedDangerousCall { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_wrong_arity_approve_errors() {
        // A mismatched approval shape must not authorize a dangerous call.
        let src = mutate_once(
            MUTATION_APPROVAL_BASE,
            "approve IssueRefund(id, amount)",
            "approve IssueRefund(id)",
        );
        let c = check(&src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UnapprovedDangerousCall { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_approve_outside_if_authorizes_inner_call() {
        // An outer approval should still authorize the inner dangerous call.
        let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund(flag: Bool, id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    if flag:
        return issue_refund(id, amount)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_nested_inner_approve_does_not_authorize_outer_call() {
        // Approval inside a nested branch must not leak outward.
        let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund(flag: Bool, id: String, amount: Float) -> Receipt:
    if flag:
        if true:
            approve IssueRefund(id, amount)
        return issue_refund(id, amount)
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UnapprovedDangerousCall { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_effect_declaration_with_dimensions_typechecks_cleanly() {
        // A declared effect row with dimensions should parse, resolve, and typecheck.
        let src = "\
effect audit_log:
    cost: $0.25
    trust: supervisor_required

tool log_refund(id: String) -> Nothing uses audit_log

agent record(id: String) -> Nothing:
    return log_refund(id)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_tool_uses_declared_effect_is_ok() {
        // A tool referencing a declared effect should resolve and typecheck cleanly.
        let src = "\
effect retrieval:
    data: grounded

tool lookup(id: String) -> Grounded<String> uses retrieval

agent load(id: String) -> Grounded<String>:
    return lookup(id)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_tool_uses_undefined_effect_is_resolution_error() {
        // Undefined effects in a uses clause must fail during resolution.
        let src = "\
tool lookup(id: String) -> String uses retrieval

agent load(id: String) -> String:
    return lookup(id)
";
        let errors = resolve_errors(src);
        assert!(errors.iter().any(|e| matches!(
            &e.kind,
            ResolveErrorKind::UndefinedName(name) if name == "retrieval"
        )), "got: {:?}", errors);
    }

    #[test]
    fn mutation_baseline_trust_violation_exists() {
        // The baseline should fail on trust: autonomous vs human_required.
        let c = check(MUTATION_EFFECT_BASE);
        assert!(has_effect_violation(&c, "trust"), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_budget_within_limit_is_ok() {
        // A budget above the composed effect cost should pass.
        let src = "\
effect transfer_money:
    cost: $0.50
    trust: human_required
    reversible: false

effect audit_log:
    cost: $0.25
    trust: supervisor_required

type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses transfer_money
tool log_refund(id: String) -> Nothing uses audit_log

@budget($1.00)
@trust(human_required)
agent ok(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    log_refund(id)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_budget_exceeded_is_effect_violation() {
        // Composed cost over budget must produce a budget violation.
        let src = "\
effect transfer_money:
    cost: $0.50
    trust: human_required
    reversible: false

effect audit_log:
    cost: $0.25
    trust: supervisor_required

type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses transfer_money
tool log_refund(id: String) -> Nothing uses audit_log

@budget($0.50)
@trust(human_required)
agent bad(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    log_refund(id)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(has_effect_violation(&c, "cost"), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_reversible_constraint_rejects_irreversible_tool() {
        // Bare @reversible must reject an irreversible call chain.
        let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

@reversible
@trust(human_required)
agent bad(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(has_effect_violation(&c, "reversible"), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_inner_agent_effects_propagate_to_outer_agent() {
        // Declared inner effects must constrain the outer caller.
        let src = "\
effect transfer_money:
    cost: $0.50
    trust: human_required
    reversible: false

type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses transfer_money

agent helper(id: String, amount: Float) -> Receipt uses transfer_money:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)

@trust(autonomous)
agent outer(id: String, amount: Float) -> Receipt:
    return helper(id, amount)
";
        let c = check(src);
        assert!(has_effect_violation(&c, "trust"), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_multiple_effects_on_one_tool_compose_cost_and_trust() {
        // Multiple effects on one tool should compose by cost-sum and trust-max.
        let src = "\
effect pay:
    cost: $0.50
    trust: autonomous

effect audit:
    cost: $0.25
    trust: supervisor_required

tool settle() -> Nothing uses pay, audit

@budget($0.60)
@trust(autonomous)
agent bad() -> Nothing:
    return settle()
";
        let c = check(src);
        assert!(has_effect_violation(&c, "cost"), "got: {:?}", c.errors);
        assert!(has_effect_violation(&c, "trust"), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_legacy_dangerous_keyword_still_works_with_dimensional_effects() {
        // Legacy dangerous must still participate when a tool also declares dimensional effects.
        let src = "\
effect audit_log:
    cost: $0.25
    trust: supervisor_required

type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses audit_log

@trust(autonomous)
agent bad(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(has_effect_violation(&c, "trust"), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_direct_grounded_return_with_retrieval_chain_is_ok() {
        // A direct retrieval source should satisfy Grounded<T> returns.
        let src = "\
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent load(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    return doc
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_grounded_return_without_retrieval_errors() {
        // Removing retrieval must be caught as an ungrounded return.
        let src = "\
tool fetch_doc(id: String) -> Grounded<String>

agent load(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    return doc
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UngroundedReturn { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_grounded_provenance_flows_through_prompts() {
        // Grounded input into a prompt should ground the prompt result.
        let c = check(MUTATION_PROVENANCE_BASE);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_ungrounded_prompt_inputs_do_not_create_grounded_output() {
        // A prompt with only ungrounded inputs must not fabricate provenance.
        let src = r#"
type Ticket:
    order_id: String

type Order:
    id: String

type Decision:
    verdict: Bool

tool get_order(id: String) -> Grounded<Order>

prompt decide(ticket: Ticket, order: Order) -> Grounded<Decision>:
    "Decide."

agent grounded_bot(ticket: Ticket) -> Grounded<Decision>:
    order = get_order(ticket.order_id)
    decision = decide(ticket, order)
    return decision
"#;
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UngroundedReturn { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_cross_agent_grounded_provenance_flows() {
        // Grounded provenance should survive an agent-to-agent hop.
        let src = r#"
effect retrieval:
    data: grounded

type Ticket:
    order_id: String

type Order:
    id: String

type Decision:
    verdict: Bool

tool get_order(id: String) -> Grounded<Order> uses retrieval

prompt decide(ticket: Ticket, order: Order) -> Grounded<Decision>:
    "Decide."

agent lookup(id: String) -> Grounded<Order>:
    return get_order(id)

agent grounded_bot(ticket: Ticket) -> Grounded<Decision>:
    order = lookup(ticket.order_id)
    decision = decide(ticket, order)
    return decision
"#;
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_intermediate_local_preserves_grounded_provenance() {
        // Passing grounded data through a local must preserve provenance.
        let src = "\
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent load(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    copy = doc
    return copy
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn mutation_missing_approve_and_ungrounded_return_report_both() {
        // Safety checks must accumulate; one violation must not hide the other.
        let src = r#"
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt summarize(id: String) -> Grounded<String>:
    "Summarize."

agent bad(id: String, amount: Float) -> Grounded<String>:
    issue_refund(id, amount)
    return summarize(id)
"#;
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UnapprovedDangerousCall { .. }
        )), "got: {:?}", c.errors);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UngroundedReturn { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_budget_and_trust_violations_report_together() {
        // Multiple dimensional violations must all be reported.
        let src = "\
effect transfer_money:
    cost: $0.75
    trust: human_required
    reversible: false

type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses transfer_money

@budget($0.50)
@trust(autonomous)
agent bad(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let c = check(src);
        assert!(has_effect_violation(&c, "cost"), "got: {:?}", c.errors);
        assert!(has_effect_violation(&c, "trust"), "got: {:?}", c.errors);
    }

    #[test]
    fn mutation_grounded_dangerous_tool_requires_approve_and_preserves_provenance() {
        // A grounded dangerous tool should satisfy provenance but still require approval.
        let src = "\
effect retrieval:
    data: grounded

tool retrieve_secret(id: String) -> Grounded<String> dangerous uses retrieval

agent bad(id: String) -> Grounded<String>:
    return retrieve_secret(id)
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UnapprovedDangerousCall { .. }
        )), "got: {:?}", c.errors);
        assert!(!c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::UngroundedReturn { .. }
        )), "got: {:?}", c.errors);
    }
}
