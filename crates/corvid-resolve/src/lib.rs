//! Name resolution for the Corvid compiler.
//!
//! Pass 1 collects every top-level declaration into a symbol table.
//! Pass 2 walks the AST and produces a side-table of `Binding`s, one
//! entry per identifier use keyed by that use's `Span`.
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

pub mod depgraph;
pub mod errors;
pub mod modules;
pub mod repl;
pub mod resolver;
pub mod scope;

pub use depgraph::{build_dep_graph, decl_name, DepGraph};
pub use errors::{ResolveError, ResolveErrorKind};
pub use modules::{
    collect_public_exports, remote_import_path, resolve_import_path, AgentSemanticSummary, DeclExport,
    ExportSemanticSummary, ImportedUseTarget, ModuleLookup, ModuleResolution,
    ModuleSemanticSummary, ResolvedModule,
};
pub use repl::{RedefinitionResult, ReplResolveSession, ResolvedTurn};
pub use resolver::{resolve, ReplayPatternBinding, Resolved};
pub use scope::{Binding, BuiltIn, DeclEntry, DeclKind, DefId, LocalId, LocalScope, SymbolTable};

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_syntax::{lex, parse_file};

    fn resolve_src(src: &str) -> Resolved {
        let tokens = lex(src).expect("lex failed");
        let (file, parse_errs) = parse_file(&tokens);
        assert!(parse_errs.is_empty(), "parse errors: {parse_errs:?}");
        resolve(&file)
    }

    #[test]
    fn resolves_simple_agent() {
        let src = "\
tool greet(name: String) -> String

agent hello(name: String) -> String:
    message = greet(name)
    return message
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "resolve errors: {:?}", r.errors);
    }

    #[test]
    fn detects_undefined_name() {
        let src = "\
agent hello(name: String) -> String:
    return unknown_function(name)
";
        let r = resolve_src(src);
        assert_eq!(r.errors.len(), 1);
        match &r.errors[0].kind {
            ResolveErrorKind::UndefinedName(n) => assert_eq!(n, "unknown_function"),
            other => panic!("expected UndefinedName, got {other:?}"),
        }
    }

    #[test]
    fn detects_duplicate_tool() {
        let src = "\
tool get_order(id: String) -> Order
tool get_order(id: String) -> Order
";
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                e.kind,
                ResolveErrorKind::DuplicateDecl { .. }
            )),
            "expected DuplicateDecl, got {:?}",
            r.errors
        );
    }

    #[test]
    fn detects_duplicate_across_categories() {
        // Strict mode: `tool foo` and `agent foo` clash even though they
        // are different declaration kinds.
        let src = "\
tool foo(x: String) -> String
agent foo(x: String) -> String:
    return x
";
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                e.kind,
                ResolveErrorKind::DuplicateDecl { .. }
            )),
            "expected DuplicateDecl across categories, got {:?}",
            r.errors
        );
    }

    #[test]
    fn parameter_is_in_scope_in_body() {
        let src = "\
agent hello(name: String) -> String:
    return name
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn local_is_in_scope_after_assignment() {
        let src = "\
agent hello(name: String) -> String:
    greeting = name
    return greeting
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn local_shadowing_allowed() {
        let src = "\
agent hello(name: String) -> String:
    name = name
    return name
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn builtin_types_resolve() {
        // `String` is a built-in; the resolver should not report it undefined.
        let src = "tool identity(x: String) -> String";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn approve_label_is_not_undefined() {
        let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let r = resolve_src(src);
        // `IssueRefund` is a label, not a declaration — must NOT be flagged.
        assert!(
            r.errors.is_empty(),
            "errors should be empty, got: {:?}",
            r.errors
        );
    }

    #[test]
    fn approve_args_are_still_resolved() {
        // If the label's args reference an unknown name, that IS an error.
        let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund(id: String) -> Receipt:
    approve IssueRefund(id, unknown_amount)
    return issue_refund(id, 1.0)
";
        let r = resolve_src(src);
        assert_eq!(r.errors.len(), 1, "expected 1 error, got: {:?}", r.errors);
        match &r.errors[0].kind {
            ResolveErrorKind::UndefinedName(n) => assert_eq!(n, "unknown_amount"),
            other => panic!("expected UndefinedName, got {other:?}"),
        }
    }

    #[test]
    fn resolves_full_refund_bot() {
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
        let r = resolve_src(src);
        assert!(
            r.errors.is_empty(),
            "full refund_bot should resolve cleanly, got: {:?}",
            r.errors
        );
        // Spot-check: the symbol table must contain all expected declarations.
        let names: Vec<&str> = r.symbols.entries().iter().map(|e| e.name.as_str()).collect();
        for expected in &[
            "anthropic",
            "Ticket",
            "Order",
            "Decision",
            "Receipt",
            "get_order",
            "issue_refund",
            "decide_refund",
            "refund_bot",
        ] {
            assert!(names.contains(expected), "missing `{expected}` in symbols: {names:?}");
        }
    }

    #[test]
    fn reassignment_reuses_same_local() {
        // `x = 1` then `x = x + 1` must bind to the same LocalId on both
        // sides — Pythonic mutation semantics. The side-table has two
        // binding entries (one per LHS span), both pointing at the same id.
        let src = "\
agent mutate(n: Int) -> Int:
    total = 0
    total = total + n
    return total
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);

        let locals: Vec<LocalId> = r
            .bindings
            .values()
            .filter_map(|b| match b {
                Binding::Local(id) => Some(*id),
                _ => None,
            })
            .collect();

        // Count distinct LocalIds used. Expected: 2 — one for `n` (param),
        // one for `total` shared between its two assignment sites and its read.
        let mut unique: Vec<LocalId> = locals.clone();
        unique.sort_by_key(|i| i.0);
        unique.dedup();
        assert_eq!(
            unique.len(),
            2,
            "expected 2 distinct LocalIds (n and total), got {}: {:?}",
            unique.len(),
            unique
        );
    }

    #[test]
    fn break_continue_pass_resolve_as_builtins() {
        let src = "\
agent loop_stuff(xs: String) -> String:
    for x in xs:
        if x:
            break
        continue
    pass
    return xs
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn bindings_side_table_is_populated() {
        let src = "\
agent echo(x: String) -> String:
    return x
";
        let r = resolve_src(src);
        // The parameter `x` and both its uses should each have a binding entry.
        // At minimum, we expect >= 2 entries (the declaration-site and the use).
        assert!(
            r.bindings.len() >= 2,
            "expected bindings for param and use, got {} entries",
            r.bindings.len()
        );
    }

    // ============================================================
    // `extend T:` method side-table tests
    // ============================================================

    #[test]
    fn extend_block_registers_methods_in_side_table() {
        let src = "\
type Order:
    amount: Int

extend Order:
    public agent total(o: Order) -> Int:
        return o.amount
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
        let order_id = r
            .symbols
            .lookup_def("Order")
            .expect("Order must be in symbols");
        let methods = r
            .methods
            .get(&order_id)
            .expect("Order must have a method table");
        assert!(methods.contains_key("total"));
        assert_eq!(
            methods["total"].kind,
            resolver::MethodKind::Agent
        );
    }

    #[test]
    fn extend_targeting_unknown_type_errors() {
        let src = "\
extend Nonexistent:
    public agent foo(x: Nonexistent) -> Int:
        return 0
";
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                e.kind,
                ResolveErrorKind::ExtendTargetNotAType(ref n) if n == "Nonexistent"
            )),
            "expected ExtendTargetNotAType, got: {:?}",
            r.errors
        );
    }

    #[test]
    fn duplicate_methods_on_same_type_errors() {
        let src = "\
type Order:
    amount: Int

extend Order:
    public agent total(o: Order) -> Int:
        return o.amount
    public agent total(o: Order) -> Int:
        return o.amount
";
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                e.kind,
                ResolveErrorKind::DuplicateMethod { ref method_name, .. } if method_name == "total"
            )),
            "expected DuplicateMethod, got: {:?}",
            r.errors
        );
    }

    #[test]
    fn method_field_collision_errors() {
        let src = "\
type Order:
    total: Int

extend Order:
    public agent total(o: Order) -> Int:
        return o.total
";
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                e.kind,
                ResolveErrorKind::MethodFieldCollision { ref method_name, .. }
                    if method_name == "total"
            )),
            "expected MethodFieldCollision, got: {:?}",
            r.errors
        );
    }

    #[test]
    fn methods_with_same_name_on_different_types_coexist() {
        let src = "\
type Order:
    amount: Int

type Line:
    amount: Int

extend Order:
    public agent total(o: Order) -> Int:
        return o.amount

extend Line:
    public agent total(l: Line) -> Int:
        return l.amount
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
        let order_id = r.symbols.lookup_def("Order").unwrap();
        let line_id = r.symbols.lookup_def("Line").unwrap();
        assert_ne!(
            r.methods[&order_id]["total"].def_id,
            r.methods[&line_id]["total"].def_id,
            "different types' methods should have distinct DefIds"
        );
    }

    // -------------------- Phase 20h: `model` decls --------------------

    #[test]
    fn resolves_model_decl_into_symbol_table() {
        let src = "\
model haiku:
    capability: basic
    cost_per_token_in: $0.00000025
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
        let def_id = r
            .symbols
            .lookup_def("haiku")
            .expect("haiku should be registered");
        let entry = r.symbols.get(def_id);
        assert_eq!(entry.kind, crate::scope::DeclKind::Model);
    }

    #[test]
    fn detects_duplicate_model_decls() {
        let src = "\
model haiku:
    capability: basic

model haiku:
    capability: expert
";
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                e.kind,
                ResolveErrorKind::DuplicateDecl { .. }
            )),
            "expected DuplicateDecl on repeated model name, got {:?}",
            r.errors
        );
    }

    #[test]
    fn model_name_collides_with_other_declaration_kinds() {
        // Models share the single top-level namespace with tools,
        // prompts, agents, effects. Same-name declarations across
        // kinds are a collision.
        let src = "\
tool haiku(q: String) -> String

model haiku:
    capability: basic
";
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                e.kind,
                ResolveErrorKind::DuplicateDecl { .. }
            )),
            "expected DuplicateDecl on tool/model name clash, got {:?}",
            r.errors
        );
    }

    #[test]
    fn multiple_distinct_models_compile_cleanly() {
        let src = "\
model haiku:
    capability: basic

model sonnet:
    capability: standard

model opus:
    capability: expert
";
        let r = resolve_src(src);
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
        assert!(r.symbols.lookup_def("haiku").is_some());
        assert!(r.symbols.lookup_def("sonnet").is_some());
        assert!(r.symbols.lookup_def("opus").is_some());
    }

    // ============================================================
    // Replay pattern resolution (21-inv-E-2b)
    // ============================================================

    #[test]
    fn replay_llm_pattern_resolves_known_prompt() {
        let src = r#"
type Decision:
    label: String

prompt classify(x: String) -> Decision:
    """Classify."""

agent check(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("fixture")
        else Decision("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.is_empty(),
            "expected clean resolve, got {:?}",
            r.errors
        );
        assert!(
            r.replay_pattern_bindings
                .values()
                .any(|b| matches!(b, ReplayPatternBinding::Llm(_))),
            "expected an Llm binding in the side-table, got {:?}",
            r.replay_pattern_bindings
        );
    }

    #[test]
    fn replay_llm_pattern_unknown_prompt_reports_error() {
        let src = r#"
type Decision:
    label: String

agent check(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("nonexistent") -> Decision("fixture")
        else Decision("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                &e.kind,
                ResolveErrorKind::UnknownReplayPrompt { name } if name == "nonexistent"
            )),
            "expected UnknownReplayPrompt, got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_llm_pattern_naming_a_tool_reports_kind_mismatch() {
        let src = r#"
type Decision:
    label: String

tool get_order(id: String) -> Decision

agent check(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("get_order") -> Decision("fixture")
        else Decision("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                &e.kind,
                ResolveErrorKind::ReplayPatternKindMismatch { name, expected_kind: "prompt", actual_kind: "tool" }
                    if name == "get_order"
            )),
            "expected ReplayPatternKindMismatch (prompt vs tool), got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_tool_pattern_resolves_known_tool() {
        let src = r#"
type Order:
    id: String

tool get_order(id: String) -> Order

agent check(x: String) -> Order:
    return replay "t.jsonl":
        when tool("get_order", _) -> Order("fixture")
        else Order("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.is_empty(),
            "expected clean resolve, got {:?}",
            r.errors
        );
        assert!(
            r.replay_pattern_bindings
                .values()
                .any(|b| matches!(b, ReplayPatternBinding::Tool(_))),
            "expected a Tool binding in the side-table"
        );
    }

    #[test]
    fn replay_tool_pattern_unknown_tool_reports_error() {
        let src = r#"
type Order:
    id: String

agent check(x: String) -> Order:
    return replay "t.jsonl":
        when tool("nonexistent", _) -> Order("fixture")
        else Order("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                &e.kind,
                ResolveErrorKind::UnknownReplayTool { name } if name == "nonexistent"
            )),
            "expected UnknownReplayTool, got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_approve_pattern_resolves_known_label() {
        let src = r#"
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return replay "t.jsonl":
        when approve("IssueRefund") -> issue_refund(id, amount)
        else issue_refund(id, amount)
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.is_empty(),
            "expected clean resolve, got {:?}",
            r.errors
        );
        assert!(
            r.replay_pattern_bindings
                .values()
                .any(|b| matches!(b, ReplayPatternBinding::Approve)),
            "expected an Approve binding in the side-table"
        );
    }

    #[test]
    fn replay_approve_pattern_unknown_label_reports_error() {
        let src = r#"
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return replay "t.jsonl":
        when approve("NonexistentSite") -> issue_refund(id, amount)
        else issue_refund(id, amount)
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                &e.kind,
                ResolveErrorKind::UnknownReplayApproval { label } if label == "NonexistentSite"
            )),
            "expected UnknownReplayApproval, got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_arm_body_can_reference_as_capture() {
        // The `as recorded` binding must be visible inside the arm
        // body; referencing it must not produce an UndefinedName.
        let src = r#"
type Decision:
    label: String

prompt classify(x: String) -> Decision:
    """Classify."""

agent check(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") as recorded -> recorded
        else Decision("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.is_empty(),
            "expected `recorded` capture to resolve cleanly, got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_arm_body_can_reference_tool_arg_capture() {
        // The bare-ident `ticket_id` in the tool arg binds a local
        // that the arm body can read.
        let src = r#"
type Order:
    id: String

tool get_order(id: String) -> Order

agent check(x: String) -> Order:
    return replay "t.jsonl":
        when tool("get_order", ticket_id) -> Order(ticket_id)
        else Order("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.is_empty(),
            "expected `ticket_id` tool-arg capture to resolve cleanly, got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_arm_captures_are_scoped_to_that_arm() {
        // A capture bound in arm 1 is NOT visible in arm 2 or in
        // the else body — each arm is its own scope.
        let src = r#"
type Decision:
    label: String

prompt classify(x: String) -> Decision:
    """Classify."""

prompt summarize(x: String) -> Decision:
    """Summarize."""

agent check(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") as recorded -> recorded
        when llm("summarize") -> recorded
        else Decision("unknown")
"#;
        let r = resolve_src(src);
        // Arm 2 references `recorded`, which was bound only in
        // arm 1 — expect an UndefinedName.
        assert!(
            r.errors.iter().any(|e| matches!(
                &e.kind,
                ResolveErrorKind::UndefinedName(n) if n == "recorded"
            )),
            "expected `recorded` to be out of scope in sibling arm, got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_trace_subexpression_resolves_as_ordinary_expr() {
        // The `<trace>` expression walks through ordinary
        // resolution; undefined names in it surface normally.
        let src = r#"
type Decision:
    label: String

prompt classify(x: String) -> Decision:
    """Classify."""

agent check(x: String) -> Decision:
    return replay unknown_var:
        when llm("classify") -> Decision("fixture")
        else Decision("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.iter().any(|e| matches!(
                &e.kind,
                ResolveErrorKind::UndefinedName(n) if n == "unknown_var"
            )),
            "expected UndefinedName for `unknown_var`, got {:?}",
            r.errors
        );
    }

    #[test]
    fn replay_arm_without_capture_does_not_regress_binding_count() {
        // Regression: arms without captures open a scope but insert
        // nothing; ensure we don't leak stale locals into neighbor
        // arms.
        let src = r#"
type Decision:
    label: String

prompt classify(x: String) -> Decision:
    """Classify."""

agent check(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("fixture")
        else Decision("unknown")
"#;
        let r = resolve_src(src);
        assert!(
            r.errors.is_empty(),
            "expected clean resolve, got {:?}",
            r.errors
        );
    }
}
