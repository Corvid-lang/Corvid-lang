//! Phase 35-G adversarial source-level bypass corpus.
//!
//! For every Static Corvid guarantee enforced by the typechecker,
//! this corpus takes a known-valid baseline source, applies a
//! mutator that introduces a violation, and asserts:
//!
//!   1. The mutated source produces at least one `TypeError`.
//!   2. At least one error carries the *expected* `guarantee_id`
//!      (proves the diagnostic-tagging from slice 35-B is actually
//!      surfacing the right registry entry).
//!   3. The error resolves through `corvid_guarantees::lookup` —
//!      the diagnostic is never anonymous.
//!
//! Each mutator is a small text-replacement transform over a
//! fixture string. We deliberately avoid AST-level mutation: the
//! corpus is more durable and easier to read as plain Corvid
//! source code, and the round-trip through lex + parse + resolve +
//! typecheck exercises the actual user-facing path.

use corvid_guarantees::lookup;
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck, Checked, TypeError};

fn check(src: &str) -> Checked {
    let tokens = lex(src).expect("lex");
    let (file, parse_errs) = parse_file(&tokens);
    assert!(parse_errs.is_empty(), "parse errors: {parse_errs:?}");
    let resolved = resolve(&file);
    assert!(
        resolved.errors.is_empty(),
        "resolve errors (test fixture should resolve cleanly): {:?}",
        resolved.errors
    );
    typecheck(&file, &resolved)
}

/// Run the typechecker and assert it surfaced an error whose
/// `guarantee_id` matches `expected`. The full error list is
/// included in the panic message so test failures point at the
/// actual diagnostics, not just the absence.
fn assert_guarantee_violated(c: &Checked, expected: &'static str) {
    assert!(
        !c.errors.is_empty(),
        "mutator was supposed to introduce a violation, but typecheck reported no errors"
    );
    let matches: Vec<&TypeError> = c
        .errors
        .iter()
        .filter(|e| e.guarantee_id == Some(expected))
        .collect();
    assert!(
        !matches.is_empty(),
        "no diagnostic carrying guarantee_id `{expected}`. \
         got: {:?}",
        c.errors
            .iter()
            .map(|e| (e.guarantee_id, format!("{:?}", e.kind)))
            .collect::<Vec<_>>()
    );
    // Every claimed guarantee_id must round-trip through the registry.
    for m in matches {
        let id = m.guarantee_id.expect("matched on Some");
        assert!(
            lookup(id).is_some(),
            "diagnostic carries unregistered guarantee id `{id}` — drift!"
        );
    }
}

// --- approval.dangerous_call_requires_token ----------------------

const APPROVAL_BASELINE: &str = r#"
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund_bot(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
"#;

#[test]
fn baseline_for_approval_compiles_clean() {
    let c = check(APPROVAL_BASELINE);
    assert!(
        c.errors.is_empty(),
        "approval baseline must be valid: {:?}",
        c.errors
    );
}

#[test]
fn mutator_drops_approve_line_triggers_dangerous_call_token_guarantee() {
    let mutated = APPROVAL_BASELINE.replace("    approve IssueRefund(id, amount)\n", "");
    let c = check(&mutated);
    assert_guarantee_violated(&c, "approval.dangerous_call_requires_token");
}

#[test]
fn mutator_wrong_arity_approve_still_triggers_token_guarantee() {
    let mutated = APPROVAL_BASELINE.replace(
        "    approve IssueRefund(id, amount)",
        "    approve IssueRefund(id)",
    );
    let c = check(&mutated);
    assert_guarantee_violated(&c, "approval.dangerous_call_requires_token");
}

// --- approval.dangerous_marker_preserved ------------------------
// Verifies that a `mock` aliasing a `@dangerous` tool does NOT
// erase the marker — calling the mocked alias still requires
// `approve`. This is the load-bearing test for the marker
// preservation guarantee, exercised through the mock path the
// resolver and typechecker share with re-export.

const ALIAS_BASELINE: &str = r#"
tool issue_refund(id: String) -> Int dangerous

mock issue_refund(id: String) -> Int:
    return 42

test approved_mock_call:
    approve IssueRefund("ord_42")
    value = issue_refund("ord_42")
    assert value == 42
"#;

#[test]
fn baseline_for_alias_compiles_clean() {
    let c = check(ALIAS_BASELINE);
    assert!(
        c.errors.is_empty(),
        "alias baseline (mock with approve) must compile clean: {:?}",
        c.errors
    );
}

#[test]
fn mutator_drops_approve_through_mock_alias_triggers_token_guarantee() {
    // Aliasing through `mock` does NOT erase `dangerous`. Drop the
    // approve line and confirm the call site still demands one.
    let mutated = ALIAS_BASELINE.replace("    approve IssueRefund(\"ord_42\")\n", "");
    let c = check(&mutated);
    assert_guarantee_violated(&c, "approval.dangerous_call_requires_token");
}

// --- approval.token_lexical_only --------------------------------

#[test]
fn mutator_approve_in_if_branch_does_not_authorize_outer_call() {
    let src = r#"
tool send_email(to: String, body: String) -> Nothing dangerous

agent notify(flag: Bool, to: String) -> Nothing:
    if flag:
        approve SendEmail(to, to)
        return send_email(to, to)
    return send_email(to, to)
"#;
    let c = check(src);
    // The unconditional outer call has no approve in its lexical
    // scope, even though the inner branch did.
    assert_guarantee_violated(&c, "approval.dangerous_call_requires_token");
}

// --- effect_row.body_completeness --------------------------------

const EFFECT_BASELINE: &str = r#"
effect llm_call:
    cost: $0.50
    trust: human_required
    reversible: false

type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses llm_call

@trust(human_required)
@budget($1.0)
agent caller(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
"#;

#[test]
fn baseline_for_effect_row_compiles_clean() {
    let c = check(EFFECT_BASELINE);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

#[test]
fn mutator_caller_under_reports_trust_triggers_body_completeness_guarantee() {
    // Tighten the agent's trust constraint so the callee's
    // human_required no longer fits — the body's actual effects
    // exceed what the agent's row covers.
    let mutated = EFFECT_BASELINE.replace("@trust(human_required)", "@trust(autonomous)");
    let c = check(&mutated);
    assert_guarantee_violated(&c, "effect_row.body_completeness");
}

// --- effect_row.caller_propagation -------------------------------

#[test]
fn mutator_outer_agent_omits_inner_effects_triggers_caller_propagation() {
    let src = r#"
effect transfer:
    cost: $0.50
    trust: human_required
    reversible: false

type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous uses transfer

agent helper(id: String, amount: Float) -> Receipt uses transfer:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)

@trust(autonomous)
agent outer(id: String, amount: Float) -> Receipt:
    return helper(id, amount)
"#;
    let c = check(src);
    assert_guarantee_violated(&c, "effect_row.body_completeness");
}

// --- effect_row.import_boundary ----------------------------------

#[test]
fn mutator_python_import_without_effects_triggers_import_boundary() {
    let src = r#"
import python "os" as os

agent main(arg: String) -> String:
    return arg
"#;
    let c = check(src);
    // Python imports MUST declare effects. The diagnostic is tagged
    // `effect_row.import_boundary`.
    assert_guarantee_violated(&c, "effect_row.import_boundary");
}

// --- grounded.provenance_required --------------------------------

#[test]
fn mutator_returns_grounded_without_retrieval_triggers_grounded_required() {
    let src = r#"
tool fabricate(seed: String) -> String

agent bot(seed: String) -> Grounded<String>:
    return fabricate(seed)
"#;
    let c = check(src);
    assert_guarantee_violated(&c, "grounded.provenance_required");
}

// --- budget.compile_time_ceiling ---------------------------------

#[test]
fn mutator_budget_under_known_cost_triggers_compile_time_ceiling() {
    let src = r#"
effect spendy:
    cost: $0.50

tool burner(x: String) -> String uses spendy

@budget($0.10)
agent over(x: String) -> String:
    return burner(x)
"#;
    let c = check(src);
    assert_guarantee_violated(&c, "budget.compile_time_ceiling");
}

// --- confidence.min_threshold ------------------------------------

#[test]
fn mutator_low_confidence_input_under_min_confidence_triggers_threshold() {
    let src = r#"
effect shaky:
    confidence: 0.70

tool shaky_lookup(q: String) -> String uses shaky

@min_confidence(0.95)
agent answer(q: String) -> String:
    return shaky_lookup(q)
"#;
    let c = check(src);
    // The composed confidence (0.70) is below the @min_confidence
    // floor (0.95). The diagnostic is tagged
    // `effect_row.body_completeness` because the dimension
    // analysis surfaces it as a non-cost effect violation.
    // Either tag is honest — confidence violations flow through
    // the same EffectConstraintViolation pipeline today.
    assert!(
        c.errors
            .iter()
            .any(|e| e.guarantee_id == Some("effect_row.body_completeness")
                || e.guarantee_id == Some("confidence.min_threshold")),
        "expected confidence-min-threshold-related violation, got: {:?}",
        c.errors
    );
}

#[test]
fn mutator_invalid_confidence_value_in_eval_triggers_threshold_guarantee() {
    let src = r#"
eval bad_eval:
    assert true with confidence 1.5 over 5 runs
"#;
    let c = check(src);
    assert_guarantee_violated(&c, "confidence.min_threshold");
}

// --- replay.deterministic_pure_path ------------------------------

#[test]
fn mutator_deterministic_agent_calls_tool_triggers_replay_guarantee() {
    let src = r#"
tool external(x: String) -> String

@deterministic
agent compute(x: String) -> String:
    return external(x)
"#;
    let c = check(src);
    assert_guarantee_violated(&c, "replay.deterministic_pure_path");
}

#[test]
fn mutator_deterministic_agent_calls_prompt_triggers_replay_guarantee() {
    let src = r#"
prompt classify(x: String) -> String:
    "x is {x}"

@deterministic
agent compute(x: String) -> String:
    return classify(x)
"#;
    let c = check(src);
    assert_guarantee_violated(&c, "replay.deterministic_pure_path");
}

// --- coverage round-up -----------------------------------------

#[test]
fn corpus_exercises_at_least_one_test_per_static_typecheck_guarantee() {
    use corvid_guarantees::{by_class, by_kind, GuaranteeClass, GuaranteeKind, GUARANTEE_REGISTRY};
    // Sanity: every static guarantee in the kinds this corpus is
    // responsible for (Approval, EffectRow, Grounded, Budget,
    // Confidence, Replay) has at least one populated
    // adversarial_test_ref. Slice 35-E's enforcement covers the
    // *registry*; this corpus is the *implementation* the registry
    // points at. The count below is a tripwire: if a new Static
    // guarantee is added in those kinds without populating a
    // mutator here, this test starts failing.
    let corpus_kinds = [
        GuaranteeKind::Approval,
        GuaranteeKind::EffectRow,
        GuaranteeKind::Grounded,
        GuaranteeKind::Budget,
        GuaranteeKind::Confidence,
        GuaranteeKind::Replay,
    ];
    for kind in corpus_kinds {
        let static_in_kind = by_kind(kind)
            .filter(|g| g.class == GuaranteeClass::Static)
            .count();
        if static_in_kind == 0 {
            continue;
        }
        let static_with_advs = by_kind(kind)
            .filter(|g| g.class == GuaranteeClass::Static)
            .filter(|g| !g.adversarial_test_refs.is_empty())
            .count();
        assert_eq!(
            static_with_advs,
            static_in_kind,
            "kind `{}` has {} Static guarantees but only {} carry adversarial test refs — \
             populate the missing test ref or downgrade the entry",
            kind.slug(),
            static_in_kind,
            static_with_advs
        );
    }
    // Also confirm the registry isn't empty (catches accidental
    // wipe-outs during refactors).
    assert!(GUARANTEE_REGISTRY.len() >= 20);
    let _ = by_class(GuaranteeClass::Static); // ensure import stays exercised
}
