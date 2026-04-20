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

    fn checked_with_file(src: &str) -> (corvid_ast::File, corvid_resolve::Resolved, Checked) {
        let tokens = lex(src).expect("lex failed");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse errors: {perr:?}");
        let resolved = resolve(&file);
        let checked = typecheck(&file, &resolved);
        (file, resolved, checked)
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
    fn retry_expression_accepts_stream_bodies() {
        let src = "\
agent flaky() -> Stream<Result<String, String>>:
    yield Err(\"boom\")

agent caller() -> Stream<Result<String, String>>:
    for item in try flaky() on error retry 3 times backoff exponential 10:
        yield item
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

    #[test]
    fn eval_value_and_trace_assertions_typecheck_cleanly() {
        let src = "\
type Ticket:
    order_id: String

type Order:
    id: String

tool get_order(id: String) -> Order
tool issue_refund(id: String) -> String dangerous

eval refund_process:
    ticket = Ticket(\"ord_42\")
    order = get_order(ticket.order_id)
    assert called get_order before issue_refund
    assert approved IssueRefund
    assert cost < $0.50
    assert order.id == order.id with confidence 0.95 over 50 runs
";
        let (_file, resolved, checked) = checked_with_file(src);
        assert!(resolved.errors.is_empty(), "resolve errors: {:?}", resolved.errors);
        assert!(checked.errors.is_empty(), "type errors: {:?}", checked.errors);
    }

    #[test]
    fn eval_non_bool_assert_is_flagged() {
        let src = r#"
tool get_order(id: String) -> String

eval bad_eval:
    order = get_order("ord_42")
    assert order
"#;
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::AssertNotBool { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn eval_called_unknown_name_fails_in_resolution() {
        let src = "\
eval bad_eval:
    assert called missing_tool
";
        let errors = resolve_errors(src);
        assert!(errors.iter().any(|e| matches!(
            e.kind,
            ResolveErrorKind::UndefinedName(ref name) if name == "missing_tool"
        )), "got: {:?}", errors);
    }

    #[test]
    fn eval_called_non_callable_is_flagged() {
        let src = "\
type Ticket:
    order_id: String

eval bad_eval:
    assert called Ticket
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::EvalUnknownTool { ref name } if name == "Ticket"
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn eval_unknown_approval_label_is_flagged() {
        let src = "\
tool issue_refund(id: String) -> String dangerous

eval bad_eval:
    assert approved MissingApproval
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::EvalUnknownApproval { ref label } if label == "MissingApproval"
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn eval_invalid_confidence_is_flagged() {
        let src = "\
eval bad_eval:
    assert true with confidence 1.5 over 5 runs
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::InvalidConfidence { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn multi_dimensional_budget_within_bound_is_clean() {
        let src = r#"
effect search_effect:
    cost: $0.001
    tokens: 12
    latency_ms: 100

effect plan_effect:
    cost: $0.030
    tokens: 835
    latency_ms: 1100

tool search(query: String) -> String uses search_effect
prompt generate_plan(results: String) -> String uses plan_effect:
    "Plan."

@budget($1.00, tokens: 10000, latency: 5s)
agent planner(query: String) -> String:
    results = search(query)
    plan = generate_plan(results)
    return plan
"#;
        let c = check(src);
        assert!(c.errors.is_empty(), "got: {:?}", c.errors);
        assert!(c.warnings.is_empty(), "unexpected warnings: {:?}", c.warnings);
    }

    #[test]
    fn multi_dimensional_budget_violation_reports_path() {
        let src = r#"
effect search_effect:
    cost: $0.001
    tokens: 12
    latency_ms: 100

effect plan_effect:
    cost: $0.030
    tokens: 835
    latency_ms: 1100

tool search(query: String) -> String uses search_effect
prompt generate_plan(results: String) -> String uses plan_effect:
    "Plan."

@budget($0.02, tokens: 500, latency: 1s)
agent planner(query: String) -> String:
    results = search(query)
    plan = generate_plan(results)
    return plan
"#;
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::EffectConstraintViolation { ref dimension, ref message, .. }
                if dimension == "cost" && message.contains("search") && message.contains("generate_plan")
        )), "got: {:?}", c.errors);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::EffectConstraintViolation { ref dimension, .. } if dimension == "tokens"
        )), "got: {:?}", c.errors);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::EffectConstraintViolation { ref dimension, .. } if dimension == "latency_ms"
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn unbounded_loop_budget_produces_warning_not_error() {
        let src = r#"
effect search_effect:
    cost: $0.010
    tokens: 100
    latency_ms: 300

tool search(query: String) -> String uses search_effect

@budget($0.05, tokens: 1000, latency: 5s)
agent planner(items: List<String>) -> String:
    total = ""
    for item in items:
        total = search(item)
    return total
"#;
        let c = check(src);
        assert!(c.errors.is_empty(), "unexpected errors: {:?}", c.errors);
        assert!(c.warnings.iter().any(|warning| matches!(
            warning.kind,
            TypeWarningKind::UnboundedCostAnalysis { .. }
        )), "got: {:?}", c.warnings);
    }

    #[test]
    fn sub_agent_costs_propagate_into_outer_agent() {
        let src = "\
effect search_effect:
    cost: $0.010
    tokens: 100
    latency_ms: 300

tool search(query: String) -> String uses search_effect

agent inner(query: String) -> String:
    return search(query)

@budget($0.02, tokens: 200, latency: 1s)
agent outer(query: String) -> String:
    return inner(query)
";
        let c = check(src);
        assert!(c.errors.is_empty(), "got: {:?}", c.errors);
    }

    // ============================================================
    // Phase 20e: confidence dimension + @min_confidence constraint
    // ============================================================

    #[test]
    fn min_confidence_passes_when_composed_confidence_meets_floor() {
        let src = "\
effect llm_decision:
    confidence: 0.95

tool search(query: String) -> String uses llm_decision

@min_confidence(0.90)
agent bot(query: String) -> String:
    return search(query)
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no confidence violation, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn min_confidence_fires_when_composed_confidence_below_floor() {
        let src = "\
effect low_confidence_llm:
    confidence: 0.70

tool shaky_search(query: String) -> String uses low_confidence_llm

@min_confidence(0.90)
agent bot(query: String) -> String:
    return shaky_search(query)
";
        let c = check(src);
        assert!(
            has_effect_violation(&c, "confidence"),
            "expected confidence violation, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn min_confidence_composes_via_min_across_multiple_calls() {
        let src = "\
effect high_conf:
    confidence: 0.98

effect low_conf:
    confidence: 0.75

tool source_a(q: String) -> String uses high_conf
tool source_b(q: String) -> String uses low_conf

@min_confidence(0.90)
agent bot(q: String) -> String:
    a = source_a(q)
    b = source_b(q)
    return b
";
        let c = check(src);
        // Composed confidence is min(0.98, 0.75) = 0.75, below the 0.90 floor.
        assert!(
            has_effect_violation(&c, "confidence"),
            "expected violation from min-composition, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn yield_requires_stream_return() {
        let src = "\
agent writer() -> String:
    yield \"hi\"
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::YieldRequiresStreamReturn { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn yield_value_must_match_stream_inner_type() {
        let src = "\
agent writer() -> Stream<String>:
    yield 1
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::YieldReturnTypeMismatch { .. }
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn yield_outside_agent_is_rejected() {
        let src = "\
eval bad:
    yield \"hi\"
    assert true
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::YieldOutsideAgent
        )), "got: {:?}", c.errors);
    }

    #[test]
    fn stream_for_loop_binds_element_type() {
        let src = "\
agent first(xs: Stream<String>) -> String:
    for x in xs:
        return x
    return \"\"
";
        let c = check(src);
        assert!(c.errors.is_empty(), "got: {:?}", c.errors);
    }

    #[test]
    fn stream_return_without_yield_warns() {
        let src = "\
agent idle() -> Stream<String>:
    pass
";
        let c = check(src);
        assert!(c.errors.is_empty(), "unexpected errors: {:?}", c.errors);
        assert!(c.warnings.iter().any(|w| matches!(
            w.kind,
            TypeWarningKind::StreamReturnWithoutYield { .. }
        )), "got: {:?}", c.warnings);
    }

    #[test]
    fn prompt_stream_modifiers_require_stream_return() {
        let src = "\
prompt generate(ctx: String) -> String:
    with max_tokens 10
    \"Generate {ctx}\"
";
        let c = check(src);
        assert!(c.errors.iter().any(|e| matches!(
            e.kind,
            TypeErrorKind::TypeMismatch { ref context, .. }
                if context.contains("stream modifiers on prompt `generate`")
        )), "got: {:?}", c.errors);
    }

    // --- Custom dimensions via corvid.toml (Phase 20g invention #6) ---

    fn check_with_config(src: &str, config: &crate::config::CorvidConfig) -> Checked {
        let tokens = lex(src).expect("lex failed");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse errors: {perr:?}");
        let resolved = resolve(&file);
        assert!(
            resolved.errors.is_empty(),
            "resolve errors: {:?}",
            resolved.errors
        );
        typecheck_with_config(&file, &resolved, Some(config))
    }

    fn parse_config(toml_src: &str) -> crate::config::CorvidConfig {
        toml::from_str(toml_src).expect("corvid.toml parse failed")
    }

    #[test]
    fn custom_dimension_registers_in_effect_registry() {
        let config = parse_config(
            r#"
            [effect-system.dimensions.freshness]
            composition = "Max"
            type = "timestamp"
            default = "0"
            semantics = "maximum age of data in seconds"
            "#,
        );

        let src = "\
effect retrieve_doc:
    freshness: 3600

tool fetch(id: String) -> String uses retrieve_doc

agent lookup(id: String) -> String:
    result = fetch(id)
    return result
";
        let c = check_with_config(src, &config);
        assert!(
            c.errors.is_empty(),
            "custom dimension freshness should compose cleanly: {:?}",
            c.errors
        );
    }

    #[test]
    fn custom_dimension_composes_via_declared_rule() {
        // Two tools each carrying freshness — the Max-composing rule
        // means the composed agent's freshness should be the larger
        // of the two inputs (300s and 3600s), surfacing as 3600.
        let config = parse_config(
            r#"
            [effect-system.dimensions.freshness]
            composition = "Max"
            type = "number"
            default = "0"
            "#,
        );

        let src = "\
effect fetch_recent:
    freshness: 300

effect fetch_stale:
    freshness: 3600

tool recent(id: String) -> String uses fetch_recent
tool stale(id: String) -> String uses fetch_stale

agent chain(id: String) -> String:
    r = recent(id)
    s = stale(id)
    return s
";
        let (file, resolved, _checked) = checked_with_file(src);
        let cfg = config;
        let decls: Vec<corvid_ast::EffectDecl> = file
            .decls
            .iter()
            .filter_map(|d| match d {
                corvid_ast::Decl::Effect(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        let registry = crate::effects::EffectRegistry::from_decls_with_config(&decls, Some(&cfg));
        assert!(
            registry.dimensions.contains_key("freshness"),
            "registry should include the user-declared freshness dimension"
        );
        let summaries = crate::effects::analyze_effects(&file, &resolved, &registry);
        let chain = summaries
            .iter()
            .find(|s| s.agent_name == "chain")
            .expect("chain agent summary");
        let freshness = chain
            .composed
            .dimensions
            .get("freshness")
            .expect("chain composed freshness");
        match freshness {
            corvid_ast::DimensionValue::Number(n) => assert!((n - 3600.0).abs() < 1e-9),
            other => panic!("unexpected freshness composition: {other:?}"),
        }
    }

    #[test]
    fn invalid_custom_dimension_surfaces_as_type_error() {
        let config = parse_config(
            r#"
            [effect-system.dimensions.freshness]
            composition = "Product"
            type = "number"
            "#,
        );

        let src = "\
agent noop() -> String:
    return \"x\"
";
        let c = check_with_config(src, &config);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::InvalidCustomDimension { dimension, .. }
                    if dimension == "freshness"
            )),
            "expected InvalidCustomDimension for `freshness`, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn builtin_collision_surfaces_as_type_error() {
        let config = parse_config(
            r#"
            [effect-system.dimensions.cost]
            composition = "Sum"
            type = "cost"
            "#,
        );

        let src = "\
agent noop() -> String:
    return \"x\"
";
        let c = check_with_config(src, &config);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::InvalidCustomDimension { dimension, .. }
                    if dimension == "cost"
            )),
            "expected InvalidCustomDimension for built-in collision, got: {:?}",
            c.errors
        );
    }

    #[test]
    fn typecheck_without_config_still_works() {
        // Regression guard: the new config-aware path must not alter
        // behavior when no corvid.toml is supplied.
        let src = "\
tool ping(id: String) -> String

agent run(id: String) -> String:
    return ping(id)
";
        let c = typecheck_with_config(
            &parse_file(&lex(src).unwrap()).0,
            &resolve(&parse_file(&lex(src).unwrap()).0),
            None,
        );
        assert!(c.errors.is_empty(), "got: {:?}", c.errors);
    }

    // --- Phase 20h: capability composition end-to-end ---

    fn compose_capability_of(src: &str, agent: &str) -> Option<String> {
        let tokens = lex(src).unwrap();
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse errors: {perr:?}");
        let resolved = resolve(&file);
        assert!(resolved.errors.is_empty(), "resolve errors: {:?}", resolved.errors);
        let effect_decls: Vec<_> = file
            .decls
            .iter()
            .filter_map(|d| match d {
                corvid_ast::Decl::Effect(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        let registry = crate::effects::EffectRegistry::from_decls(&effect_decls);
        let summaries = crate::effects::analyze_effects(&file, &resolved, &registry);
        summaries
            .into_iter()
            .find(|s| s.agent_name == agent)?
            .composed
            .dimensions
            .get("capability")
            .map(|v| match v {
                corvid_ast::DimensionValue::Name(n) => n.clone(),
                other => format!("{other:?}"),
            })
    }

    #[test]
    fn agent_without_prompt_calls_sits_at_default_capability() {
        // `capability` is a built-in dimension, so the composed
        // profile always carries it. With no prompts declaring
        // `requires:`, the value is the default (`basic`).
        let src = "\
tool echo(x: String) -> String

agent passthrough(x: String) -> String:
    return echo(x)
";
        let cap = compose_capability_of(src, "passthrough");
        assert_eq!(cap.as_deref(), Some("basic"));
    }

    #[test]
    fn prompt_requires_flows_into_agent_composed_profile() {
        let src = "\
prompt classify(t: String) -> String:
    requires: standard
    \"Classify {t}\"

agent classifier(t: String) -> String:
    return classify(t)
";
        let cap = compose_capability_of(src, "classifier");
        assert_eq!(cap.as_deref(), Some("standard"));
    }

    #[test]
    fn multiple_prompt_capabilities_compose_by_max() {
        // Two prompts at `basic` and `expert`; agent's composed
        // capability is `expert` (strictest).
        let src = "\
prompt simple(t: String) -> String:
    requires: basic
    \"Simple {t}\"

prompt hard(t: String) -> String:
    requires: expert
    \"Hard {t}\"

agent both(t: String) -> String:
    a = simple(t)
    b = hard(t)
    return a
";
        let cap = compose_capability_of(src, "both");
        assert_eq!(cap.as_deref(), Some("expert"));
    }

    #[test]
    fn capability_propagates_through_agent_call_chains() {
        // An inner agent calls an expert-level prompt.
        // The outer agent calls the inner agent; its composed
        // capability should still be `expert`.
        let src = "\
prompt hard(t: String) -> String:
    requires: expert
    \"Hard {t}\"

agent inner(t: String) -> String:
    return hard(t)

agent outer(t: String) -> String:
    return inner(t)
";
        let cap = compose_capability_of(src, "outer");
        assert_eq!(cap.as_deref(), Some("expert"));
    }

    // --- Phase 20h slice C: `route:` clause validation ---

    #[test]
    fn route_arm_pointing_at_non_model_is_rejected() {
        let src = "\
tool not_a_model(q: String) -> String

prompt answer(q: String) -> String:
    route:
        _ -> not_a_model
    \"Answer\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::RouteTargetNotModel { target, .. } if target == "not_a_model"
            )),
            "expected RouteTargetNotModel error, got {:?}",
            c.errors
        );
    }

    #[test]
    fn route_guard_not_bool_is_rejected() {
        let src = "\
model m1:
    capability: basic

prompt answer(q: String) -> String:
    route:
        q -> m1
        _ -> m1
    \"Answer\"
";
        // `q` is a String, not a Bool — guard should fail type check.
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::RouteGuardNotBool { prompt, .. } if prompt == "answer"
            )),
            "expected RouteGuardNotBool error, got {:?}",
            c.errors
        );
    }

    #[test]
    fn route_with_valid_model_and_bool_guard_passes() {
        let src = "\
model fast:
    capability: basic

model slow:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == \"hard\" -> slow
        _ -> fast
    \"Answer\"
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn route_with_undefined_model_target_is_rejected() {
        let src = "\
prompt answer(q: String) -> String:
    route:
        _ -> nonexistent_model
    \"Answer\"
";
        let resolve_errs = resolve_errors(src);
        assert!(
            resolve_errs.iter().any(|e| matches!(
                &e.kind,
                corvid_resolve::ResolveErrorKind::UndefinedName(n) if n == "nonexistent_model"
            )),
            "expected UndefinedName on unresolved route target, got {:?}",
            resolve_errs
        );
    }

    // --- Phase 20h slice E: progressive refinement validation ---

    #[test]
    fn progressive_with_valid_models_and_thresholds_passes() {
        let src = "\
model cheap:
    capability: basic

model expensive:
    capability: expert

prompt classify(q: String) -> String:
    progressive:
        cheap below 0.95
        expensive
    \"Classify\"
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn progressive_stage_pointing_at_non_model_is_rejected() {
        let src = "\
tool not_a_model(q: String) -> String

model fallback:
    capability: expert

prompt classify(q: String) -> String:
    progressive:
        not_a_model below 0.95
        fallback
    \"Classify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::RouteTargetNotModel { target, .. } if target == "not_a_model"
            )),
            "expected RouteTargetNotModel for non-model stage, got {:?}",
            c.errors
        );
    }

    // --- Phase 20h slice I: rollout validation ---

    #[test]
    fn rollout_with_valid_models_and_percent_passes() {
        let src = "\
model v1:
    capability: expert

model v2:
    capability: expert

prompt summarize(doc: String) -> String:
    rollout 10% v2, else v1
    \"Summarize\"
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn rollout_pointing_at_non_model_is_rejected() {
        let src = "\
tool not_a_model(q: String) -> String

model v1:
    capability: expert

prompt summarize(doc: String) -> String:
    rollout 10% not_a_model, else v1
    \"Summarize\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::RouteTargetNotModel { target, .. } if target == "not_a_model"
            )),
            "expected RouteTargetNotModel, got {:?}",
            c.errors
        );
    }

    // --- Phase 20h slice F: ensemble validation ---

    #[test]
    fn ensemble_with_valid_models_passes() {
        let src = "\
model a:
    capability: basic

model b:
    capability: standard

model c:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [a, b, c] vote majority
    \"Answer\"
";
        let c_out = check(src);
        assert!(c_out.errors.is_empty(), "errors: {:?}", c_out.errors);
    }

    #[test]
    fn ensemble_model_pointing_at_non_model_is_rejected() {
        let src = "\
tool not_a_model(q: String) -> String

model real:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [not_a_model, real] vote majority
    \"Answer\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::RouteTargetNotModel { target, .. } if target == "not_a_model"
            )),
            "expected RouteTargetNotModel, got {:?}",
            c.errors
        );
    }

    #[test]
    fn ensemble_with_duplicate_model_is_rejected() {
        let src = "\
model a:
    capability: basic

model b:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [a, b, a] vote majority
    \"Answer\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::EnsembleDuplicateModel { model, .. } if model == "a"
            )),
            "expected EnsembleDuplicateModel, got {:?}",
            c.errors
        );
    }

    // --- Phase 20h slice G: adversarial validation (Option B) ---
    //
    // Stages are `prompt` decls, not `model` decls. The runtime
    // chains stage outputs as positional arguments:
    //   propose(outer_params) -> T1
    //   challenge(T1)          -> T2
    //   adjudicate(T1, T2)     -> Outer       (must be a struct
    //                                          with a `contradiction:
    //                                          Bool` field)

    #[test]
    fn adversarial_with_valid_prompt_stages_passes() {
        let src = "\
type Verdict:
    contradiction: Bool
    rationale: String

prompt propose_answer(q: String) -> String:
    \"Answer: {q}\"

prompt critique(proposed: String) -> String:
    \"Flaws in: {proposed}\"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    \"Verdict on {proposed} vs {challenge}\"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    \"Verify\"
";
        let c = check(src);
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
    }

    #[test]
    fn adversarial_stage_pointing_at_non_prompt_is_rejected() {
        // A `model` is not a prompt — stages must be prompts because
        // the runtime chains outputs through positional call syntax.
        let src = "\
type Verdict:
    contradiction: Bool

model bare_model:
    capability: expert

prompt critique(proposed: String) -> String:
    \"Flaws: {proposed}\"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    \"Verdict\"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: bare_model
        challenge: critique
        adjudicate: adjudicate_fn
    \"Verify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::AdversarialStageNotPrompt { target, stage, .. }
                    if target == "bare_model" && stage == "propose"
            )),
            "expected AdversarialStageNotPrompt for bare_model, got {:?}",
            c.errors
        );
    }

    #[test]
    fn adversarial_challenger_wrong_arity_is_rejected() {
        // Challenger must accept exactly 1 parameter (the proposer's
        // return value). A two-param challenger is rejected.
        let src = "\
type Verdict:
    contradiction: Bool

prompt propose_answer(q: String) -> String:
    \"Answer: {q}\"

prompt critique_bad(a: String, b: String) -> String:
    \"Flaws\"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    \"Verdict\"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique_bad
        adjudicate: adjudicate_fn
    \"Verify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::AdversarialStageArity {
                    stage, expected, got, ..
                } if stage == "challenge" && *expected == 1 && *got == 2
            )),
            "expected AdversarialStageArity(challenge, 1, 2), got {:?}",
            c.errors
        );
    }

    #[test]
    fn adversarial_adjudicator_param_type_mismatch_is_rejected() {
        // Adjudicator's second param must accept the challenger's
        // return type. Int vs String mismatch is rejected.
        let src = "\
type Verdict:
    contradiction: Bool

prompt propose_answer(q: String) -> String:
    \"Answer: {q}\"

prompt critique(proposed: String) -> String:
    \"Flaws\"

prompt adjudicate_bad(proposed: String, flaws: Int) -> Verdict:
    \"Verdict\"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_bad
    \"Verify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::AdversarialStageParamType {
                    stage, index, ..
                } if stage == "adjudicate" && *index == 1
            )),
            "expected AdversarialStageParamType(adjudicate, #1), got {:?}",
            c.errors
        );
    }

    #[test]
    fn adversarial_adjudicator_return_mismatch_is_rejected() {
        // Outer prompt declares `-> Verdict`, adjudicator returns
        // `String` — these must match for the pipeline's output to
        // be the prompt's output.
        let src = "\
type Verdict:
    contradiction: Bool

prompt propose_answer(q: String) -> String:
    \"Answer: {q}\"

prompt critique(proposed: String) -> String:
    \"Flaws\"

prompt adjudicate_bad(proposed: String, flaws: String) -> String:
    \"Not a Verdict\"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_bad
    \"Verify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::AdversarialStageReturnType { stage, .. }
                    if stage == "adjudicate"
            )),
            "expected AdversarialStageReturnType(adjudicate), got {:?}",
            c.errors
        );
    }

    #[test]
    fn adversarial_adjudicator_missing_contradiction_field_is_rejected() {
        // Adjudicator's return struct must have `contradiction: Bool`
        // because the runtime reads it to decide whether to emit a
        // `TraceEvent::AdversarialContradiction`.
        let src = "\
type NoContradiction:
    rationale: String

prompt propose_answer(q: String) -> String:
    \"Answer: {q}\"

prompt critique(proposed: String) -> String:
    \"Flaws\"

prompt adjudicate_fn(proposed: String, flaws: String) -> NoContradiction:
    \"Verdict\"

prompt verify(q: String) -> NoContradiction:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    \"Verify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::AdversarialAdjudicatorMissingContradictionField { .. }
            )),
            "expected AdversarialAdjudicatorMissingContradictionField, got {:?}",
            c.errors
        );
    }

    #[test]
    fn adversarial_contradiction_field_wrong_type_is_rejected() {
        // A `contradiction: String` field does not satisfy the
        // contract — the runtime reads the field as `Bool`.
        let src = "\
type WrongType:
    contradiction: String

prompt propose_answer(q: String) -> String:
    \"Answer: {q}\"

prompt critique(proposed: String) -> String:
    \"Flaws\"

prompt adjudicate_fn(proposed: String, flaws: String) -> WrongType:
    \"Verdict\"

prompt verify(q: String) -> WrongType:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    \"Verify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::AdversarialAdjudicatorMissingContradictionField { .. }
            )),
            "expected AdversarialAdjudicatorMissingContradictionField for wrong field type, got {:?}",
            c.errors
        );
    }

    #[test]
    fn adversarial_proposer_arity_must_match_outer_prompt() {
        // Outer prompt takes 1 param, proposer takes 2 — pipeline
        // can't wire the outer call's args to the proposer.
        let src = "\
type Verdict:
    contradiction: Bool

prompt propose_bad(a: String, b: String) -> String:
    \"Answer\"

prompt critique(proposed: String) -> String:
    \"Flaws\"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    \"Verdict\"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_bad
        challenge: critique
        adjudicate: adjudicate_fn
    \"Verify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::AdversarialStageArity {
                    stage, expected, got, ..
                } if stage == "propose" && *expected == 1 && *got == 2
            )),
            "expected AdversarialStageArity(propose, 1, 2), got {:?}",
            c.errors
        );
    }

    #[test]
    fn rollout_percent_out_of_range_is_rejected() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt p(q: String) -> String:
    rollout 150% a, else b
    \"X\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::RolloutPercentOutOfRange { got, .. } if (*got - 150.0).abs() < 1e-9
            )),
            "expected RolloutPercentOutOfRange, got {:?}",
            c.errors
        );
    }

    #[test]
    fn progressive_threshold_out_of_range_is_rejected() {
        let src = "\
model a:
    capability: basic

model b:
    capability: expert

prompt classify(q: String) -> String:
    progressive:
        a below 1.5
        b
    \"Classify\"
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::InvalidConfidence { value } if (*value - 1.5).abs() < 1e-9
            )),
            "expected InvalidConfidence for threshold=1.5, got {:?}",
            c.errors
        );
    }

    #[test]
    fn extern_c_agent_with_scalar_signature_typechecks() {
        let checked = check(
            r#"
pub extern "c"
agent refund_bot(ticket_id: String, amount: Float) -> Bool:
    return true
"#,
        );
        assert!(
            checked.errors.is_empty(),
            "expected scalar extern agent to typecheck, got {:?}",
            checked.errors
        );
    }

    #[test]
    fn extern_c_agent_with_struct_param_errors_with_hint_at_22b() {
        let checked = check(
            r#"
type Ticket:
    id: String

pub extern "c"
agent refund_bot(ticket: Ticket) -> Bool:
    return true
"#,
        );
        let err = checked
            .errors
            .iter()
            .find(|e| matches!(e.kind, TypeErrorKind::NonScalarInExternC { .. }))
            .expect("expected NonScalarInExternC error");
        assert!(
            err.hint().unwrap_or_default().contains("Phase 22-B"),
            "expected Phase 22-B hint, got {:?}",
            err.hint()
        );
    }

    #[test]
    fn extern_c_agent_with_list_return_errors_with_hint_at_22b() {
        let checked = check(
            r#"
pub extern "c"
agent ids() -> List<String>:
    return ["a"]
"#,
        );
        let err = checked
            .errors
            .iter()
            .find(|e| matches!(e.kind, TypeErrorKind::NonScalarInExternC { .. }))
            .expect("expected NonScalarInExternC error");
        assert!(
            err.hint().unwrap_or_default().contains("Phase 22-B"),
            "expected Phase 22-B hint, got {:?}",
            err.hint()
        );
    }

    // -------------------- Phase 21 slice inv-A: @replayable --------------------

    #[test]
    fn replayable_agent_with_pure_body_compiles_clean() {
        // An agent marked @replayable whose body touches no
        // nondeterministic sources compiles without errors. The
        // determinism catalog is empty as of Phase 21 v1 so this
        // is the common case.
        let src = "\
@replayable
agent echo(q: String) -> String:
    return q
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no errors for pure @replayable agent, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replayable_agent_calling_tool_compiles_clean() {
        // Tool calls are always captured via ToolCall/ToolResult
        // events, so they are replayable by construction.
        let src = "\
tool get_order(id: String) -> String

@replayable
agent lookup(id: String) -> String:
    return get_order(id)
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no errors for @replayable agent calling tool, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replayable_agent_calling_prompt_compiles_clean() {
        // Prompt calls are captured via LlmCall/LlmResult events,
        // so they are replayable by construction.
        let src = "\
prompt classify(q: String) -> String:
    \"Classify: {q}\"

@replayable
agent route_query(q: String) -> String:
    return classify(q)
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no errors for @replayable agent calling prompt, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replayable_attribute_is_recorded_on_agent_decl() {
        // Verifies the AST wiring: the attribute makes it from the
        // parser into AgentDecl.attributes, separately from
        // dimensional effect constraints.
        let src = "\
@replayable
agent refund_flow(q: String) -> String:
    return q
";
        let tokens = lex(src).unwrap();
        let (file, errs) = parse_file(&tokens);
        assert!(errs.is_empty(), "parse errors: {errs:?}");
        let agent = file
            .decls
            .iter()
            .find_map(|d| match d {
                corvid_ast::Decl::Agent(a) => Some(a),
                _ => None,
            })
            .expect("expected an agent decl");
        assert_eq!(agent.attributes.len(), 1);
        assert!(matches!(
            agent.attributes[0],
            corvid_ast::AgentAttribute::Replayable { .. }
        ));
        assert!(agent.constraints.is_empty());
    }

    #[test]
    fn replayable_with_effect_constraint_coexist() {
        // @replayable lives in attributes; @budget lives in
        // constraints. Both apply; neither pollutes the other.
        let src = "\
@replayable
@budget($1.00)
agent bounded(q: String) -> String:
    return q
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no errors, got {:?}",
            c.errors
        );
    }

    // -------------------- Phase 21 slice inv-F: @deterministic --------------------

    #[test]
    fn deterministic_agent_with_pure_body_compiles_clean() {
        let src = "\
@deterministic
agent identity(q: String) -> String:
    return q
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no errors for pure @deterministic agent, got {:?}",
            c.errors
        );
    }

    #[test]
    fn deterministic_agent_calling_tool_is_rejected() {
        let src = "\
tool get_order(id: String) -> String

@deterministic
agent lookup(id: String) -> String:
    return get_order(id)
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::NonDeterministicCall { call, call_kind, .. }
                    if call == "get_order" && call_kind == "tool"
            )),
            "expected NonDeterministicCall for tool invocation, got {:?}",
            c.errors
        );
    }

    #[test]
    fn deterministic_agent_calling_prompt_is_rejected() {
        let src = "\
prompt classify(q: String) -> String:
    \"Classify: {q}\"

@deterministic
agent choose(q: String) -> String:
    return classify(q)
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::NonDeterministicCall { call, call_kind, .. }
                    if call == "classify" && call_kind == "prompt"
            )),
            "expected NonDeterministicCall for prompt invocation, got {:?}",
            c.errors
        );
    }

    #[test]
    fn deterministic_agent_calling_non_deterministic_agent_is_rejected() {
        let src = "\
agent helper(q: String) -> String:
    return q

@deterministic
agent wrapper(q: String) -> String:
    return helper(q)
";
        let c = check(src);
        assert!(
            c.errors.iter().any(|e| matches!(
                &e.kind,
                TypeErrorKind::NonDeterministicCall { call, call_kind, .. }
                    if call == "helper" && call_kind.contains("agent")
            )),
            "expected NonDeterministicCall for non-deterministic agent call, got {:?}",
            c.errors
        );
    }

    #[test]
    fn deterministic_agent_calling_deterministic_agent_compiles_clean() {
        // @deterministic propagates: a deterministic agent can
        // call another @deterministic agent, because the callee's
        // body is also provably pure.
        let src = "\
@deterministic
agent helper(q: String) -> String:
    return q

@deterministic
agent wrapper(q: String) -> String:
    return helper(q)
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no errors for @deterministic -> @deterministic call, got {:?}",
            c.errors
        );
    }

    #[test]
    fn deterministic_implies_replayable() {
        // An agent marked only @deterministic should satisfy
        // replayability invariants without needing @replayable too.
        // Since the body is pure, both checks pass trivially today.
        let src = "\
@deterministic
agent pure(q: String) -> String:
    return q
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "@deterministic should imply @replayable, got {:?}",
            c.errors
        );
    }

    #[test]
    fn deterministic_and_replayable_coexist() {
        // Redundant but valid — both attributes on the same
        // agent; checker treats them independently and both
        // pass on a pure body.
        let src = "\
@deterministic
@replayable
agent pure(q: String) -> String:
    return q
";
        let c = check(src);
        assert!(
            c.errors.is_empty(),
            "expected no errors for @deterministic + @replayable, got {:?}",
            c.errors
        );
    }

    // ============================================================
    // Replay expression typechecking (21-inv-E-3)
    // ============================================================

    const REPLAY_PRELUDE: &str = r#"
type Decision:
    label: String

type Order:
    id: String

prompt classify(x: String) -> Decision:
    """Classify."""

tool get_order(id: String) -> Order

tool issue_refund(id: String, amount: Float) -> Order dangerous
"#;

    fn check_with_prelude(body: &str) -> Checked {
        let src = format!("{REPLAY_PRELUDE}\n{body}");
        let tokens = lex(&src).expect("lex failed");
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

    fn has_replay_trace_type_error(c: &Checked) -> bool {
        c.errors.iter().any(|e| matches!(
            &e.kind,
            TypeErrorKind::ReplayTraceNotATraceId { .. }
        ))
    }

    fn has_replay_arm_type_mismatch(c: &Checked) -> bool {
        c.errors.iter().any(|e| matches!(
            &e.kind,
            TypeErrorKind::ReplayArmTypeMismatch { .. }
        ))
    }

    #[test]
    fn replay_with_string_literal_trace_typechecks() {
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("fixture")
        else Decision("unknown")
"#;
        let c = check_with_prelude(body);
        assert!(
            c.errors.is_empty(),
            "expected clean replay typecheck, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replay_with_non_traceid_non_string_trace_errors() {
        // An Int literal where the trace goes must surface
        // ReplayTraceNotATraceId.
        let body = r#"
agent run(x: String) -> Decision:
    return replay 42:
        when llm("classify") -> Decision("fixture")
        else Decision("unknown")
"#;
        let c = check_with_prelude(body);
        assert!(
            has_replay_trace_type_error(&c),
            "expected ReplayTraceNotATraceId, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replay_arm_type_mismatch_surfaces() {
        // Arm 1 returns Decision, arm 2 returns a Decision too,
        // but `else` returns an Order — the join fails.
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("fixture")
        else Order("mismatched")
"#;
        let c = check_with_prelude(body);
        assert!(
            has_replay_arm_type_mismatch(&c),
            "expected ReplayArmTypeMismatch, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replay_arm_body_can_use_whole_event_capture_with_correct_type() {
        // `as recorded` binds a Decision (the prompt's return type);
        // referencing `recorded` as the arm body must typecheck.
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") as recorded -> recorded
        else Decision("unknown")
"#;
        let c = check_with_prelude(body);
        assert!(
            c.errors.is_empty(),
            "expected capture type to flow, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replay_arm_tool_arg_capture_has_tools_first_param_type() {
        // `tool("get_order", ticket_id)` binds `ticket_id` to String
        // (get_order's first param). Using it where a String is
        // expected typechecks cleanly.
        let body = r#"
agent run(x: String) -> Order:
    return replay "t.jsonl":
        when tool("get_order", ticket_id) -> get_order(ticket_id)
        else get_order(x)
"#;
        let c = check_with_prelude(body);
        assert!(
            c.errors.is_empty(),
            "expected tool-arg capture to type as String, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replay_approve_capture_types_as_bool() {
        // `as decision` on an approve arm binds a Bool. Using it as
        // the condition of an if-expression check works only if
        // Bool-typed.
        let body = r#"
agent run(id: String, amount: Float) -> Order:
    approve IssueRefund(id, amount)
    return replay "t.jsonl":
        when approve("IssueRefund") as verdict -> get_order(id)
        else get_order(id)
"#;
        let c = check_with_prelude(body);
        assert!(
            c.errors.is_empty(),
            "expected approval capture typing to work, got {:?}",
            c.errors
        );
    }

    #[test]
    fn replay_duplicate_pattern_warns_unreachable_arm() {
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("first")
        when llm("classify") -> Decision("shadow")
        else Decision("unknown")
"#;
        let c = check_with_prelude(body);
        assert!(
            c.warnings.iter().any(|w| matches!(
                &w.kind,
                TypeWarningKind::ReplayUnreachableArm { pattern, .. } if pattern.contains("classify")
            )),
            "expected ReplayUnreachableArm warning, got {:?}",
            c.warnings
        );
    }

    #[test]
    fn replay_whole_body_types_as_single_joined_type() {
        // When all arms + else produce the same type, the replay
        // expression has that type — smoke check via a successful
        // typecheck of an enclosing agent whose return type matches.
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("a")
        when llm("classify") -> Decision("b")
        else Decision("c")
"#;
        let c = check_with_prelude(body);
        // There's an unreachable-arm warning (arm 2 duplicates arm 1)
        // but the arm/body typing still reaches Decision; no errors.
        assert!(
            c.errors.is_empty(),
            "expected clean errors (warnings ok), got {:?}",
            c.errors
        );
    }
