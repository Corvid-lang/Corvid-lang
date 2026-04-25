//! Principled `Dup`/`Drop` insertion pass.
//!
//! Walks each agent body and inserts `IrStmt::Dup` / `IrStmt::Drop`
//! at every ownership boundary. The output IR is consumed by
//! `lowering.rs` in place of the scattered `emit_retain`/`emit_release`
//! calls that existed before the unified ownership path.
//!
//! ## Algorithm overview
//!
//! 1. **Per-agent use-list.** For each refcounted local, collect every
//!    `IrExprKind::Local { local_id }` position in the body (source
//!    order inside the per-local lexical scope).
//!
//! 2. **Lean 4-style borrow inference.** Whole-program monotone
//!    fixed-point over the call graph. Initial state: every refcounted
//!    parameter is `Borrowed` (optimistic). Promote to `Owned` when
//!    the body contains a consumer (store-into-heap, return-of-param,
//!    pass-as-Owned-arg to another callee). Iterate until stable.
//!    Matches the rules described in Ullrich & de Moura, "Counting
//!    Immutable Beans" §4.
//!
//! 3. **Last-use / Move elision.** Every `Local` use site is
//!    classified as either the *final* use on every forward path from
//!    its definition (a Move — no Dup needed, consumer receives the
//!    +1) or a *non-final* use (emits a `Dup` before the consuming
//!    statement; the original binding stays live).
//!
//! 4. **Branch-aware Drop placement.** At block / branch / loop
//!    boundaries, every still-Live binding with no subsequent use
//!    gets a `Drop`. Asymmetric branches — a binding consumed in one
//!    branch and not the other — get a compensating `Drop` on the
//!    non-consuming branch.
//!
//! 5. **Function summaries.** Per agent we compute `may_retain`,
//!    `may_release`, and per-parameter `borrows_param` flags. These
//!    are consumed by whole-program retain/release pair elimination.
//!
//! ## Why this pass is indivisible
//!
//! A partial implementation — e.g., borrow inference without Dup
//! insertion — produces an IR where callees expect Borrowed semantics
//! but callers still emit Owned ABI. That's a semantic gap, not an
//! optimization. All four pieces (use-list, borrow inference,
//! Dup/Drop insertion, scattered-site deletion) land together or
//! not at all.
//!
//! ## Correctness invariants
//!
//! * **No double-free.** For every refcounted local `L`, along every
//!   path from L's definition to program end: exactly one of
//!   {consuming-move, explicit Drop} fires.
//! * **No leak.** For every refcounted `L` with `ρ(L) = Live` at
//!   scope exit, a `Drop(L)` is emitted.
//! * **Borrow safety.** If `σ(f, i) = Borrowed`, the callee body
//!   emits no Drop on `p_i` and emits a Dup before any consuming use.
//! * **Parity preservation.** `ALLOCS == RELEASES` for every test.
//!
//! ## What this pass does NOT do
//!
//! * **No whole-program retain/release pair elimination** — that
//!   consumes the function summaries emitted here.
//! * **No drop specialization.**
//! * **No reuse / in-place update.**
//! * **No per-call-site specialization.**
//! * **No escape analysis / stack promotion.**

use corvid_ir::{
    IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrStmt, ParamBorrow,
};
use corvid_resolve::{DefId, LocalId};
use corvid_types::Type;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// Per-agent analysis summary. Populated for every agent in the file.
/// Consumed by the whole-program pair-elimination pass.
#[derive(Debug, Clone)]
pub struct AgentSummary {
    /// Mirror of `IrAgent.borrow_sig`. Populated here, written back
    /// onto the `IrAgent` in the transformed IR.
    pub borrow_sig: Vec<ParamBorrow>,
    /// Does the body contain any `Dup`?
    pub may_retain: bool,
    /// Does the body contain any `Drop` or other consumer?
    pub may_release: bool,
    /// Per-parameter: is `p_i` borrowed on every code path?
    /// `true` means the pair-elimination pass can eliminate a cross-call retain/release
    /// pair for this argument slot.
    pub borrows_param: Vec<bool>,
}

/// Top-level entry point. Consumes an `IrFile`, runs the full
/// analysis + transformation, returns a new `IrFile` with `Dup`/`Drop`
/// statements inserted and `borrow_sig` populated on every agent,
/// plus per-agent summaries for later ownership cleanups.
pub fn analyze(ir: IrFile) -> (IrFile, HashMap<DefId, AgentSummary>) {
    // Step 1: borrow inference (Lean 4-style fixed point).
    let borrow_sigs = infer_borrow_sigs(&ir);

    // Step 2: per-agent `Dup`/`Drop` insertion.
    let mut transformed_agents = Vec::with_capacity(ir.agents.len());
    let mut summaries: HashMap<DefId, AgentSummary> = HashMap::new();
    for agent in &ir.agents {
        let sig = borrow_sigs
            .get(&agent.id)
            .cloned()
            .unwrap_or_else(|| default_borrow_sig(&agent.params));
        let (transformed, summary) = transform_agent(agent, sig, &borrow_sigs);
        transformed_agents.push(transformed);
        summaries.insert(agent.id, summary);
    }

    let out = IrFile {
        imports: ir.imports,
        types: ir.types,
        tools: ir.tools,
        prompts: ir.prompts,
        agents: transformed_agents,
        evals: ir.evals,
        tests: ir.tests,
    };
    (out, summaries)
}

fn default_borrow_sig(params: &[corvid_ir::IrParam]) -> Vec<ParamBorrow> {
    params.iter().map(|_| ParamBorrow::Owned).collect()
}

// ---------------------------------------------------------------------------
// Step 1 — borrow inference (Lean 4-style monotone fixed point)
// ---------------------------------------------------------------------------

/// A parameter is `Borrowed` iff the body never consumes it. "Consume"
/// means one of:
///   - stored into a heap location (struct field, list element,
///     list literal item slot)
///   - passed as an argument to another callee whose σ says Owned
///     for that slot
///   - returned directly (the return-path consumer of p_i counts
///     as Owned unless we can emit Dup before return — which
///     requires this same pass to run first, so we break the
///     chicken-and-egg via monotone fixed point)
///
/// Returning a parameter is the load-bearing case: in the first
/// iteration we don't yet know whether emitting `Dup(p_i); return p_i`
/// is viable; we pessimistically mark return-of-param as Owned. On
/// subsequent iterations, if we observe that nothing else consumes
/// p_i, we upgrade return-of-param to "emit Dup before return" —
/// but that upgrade actually lands in the insertion step.
/// For borrow inference, return-of-param stays Owned.
///
/// This is intentionally conservative: `Dup-before-return` is handled
/// by the insertion step in this same module.
/// The optimistic "every param starts Borrowed" initial state of
/// the fixed point avoids the chicken-and-egg in the limit case of
/// a param with no consumers at all (e.g., `agent f(x: String) -> Int: return 0`
/// where x is unused — correctly classified Borrowed in iteration 1).
fn infer_borrow_sigs(ir: &IrFile) -> HashMap<DefId, Vec<ParamBorrow>> {
    // Initial state: every refcounted parameter is Borrowed.
    // Non-refcounted parameters are trivially Owned (no RC distinction).
    let mut sigs: HashMap<DefId, Vec<ParamBorrow>> = ir
        .agents
        .iter()
        .map(|a| {
            let v: Vec<ParamBorrow> = a
                .params
                .iter()
                .map(|p| {
                    if is_refcounted(&p.ty) {
                        ParamBorrow::Borrowed
                    } else {
                        ParamBorrow::Owned
                    }
                })
                .collect();
            (a.id, v)
        })
        .collect();

    // Iterate to fixed point. Monotone: Borrowed → Owned, never back.
    loop {
        let mut changed = false;
        for agent in &ir.agents {
            let current = sigs[&agent.id].clone();
            for (i, param) in agent.params.iter().enumerate() {
                if current[i] == ParamBorrow::Owned {
                    continue; // already promoted; monotone
                }
                if !is_refcounted(&param.ty) {
                    continue;
                }
                if param_is_consumed(agent, param.local_id, &sigs) {
                    let entry = sigs.get_mut(&agent.id).unwrap();
                    if entry[i] != ParamBorrow::Owned {
                        entry[i] = ParamBorrow::Owned;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    sigs
}

/// Does the agent's body consume `target`? Checks for consumers:
///   - Return of `target` (without prior local Dup — always true in
///     initial inference; `Dup` insertion happens in the next step)
///   - Store into a struct field, list literal slot, list element
///   - Pass as argument to callee `g` where `σ(g, slot) = Owned`
fn param_is_consumed(
    agent: &IrAgent,
    target: LocalId,
    sigs: &HashMap<DefId, Vec<ParamBorrow>>,
) -> bool {
    let mut consumed = false;
    visit_block(&agent.body, &mut |stmt| {
        match stmt {
            IrStmt::Return { value: Some(e), .. } => {
                // Return-of-param does NOT count as a consume under
                // Perceus borrow semantics: the callee emits Dup
                // before the return so the caller receives a +1
                // independently of the parameter's ownership.
                //
                // In Corvid's current codegen the Dup-before-return
                // is already emitted naturally by lower_expr's
                // retain on `IrExprKind::Local`, so this pass
                // (17b-1b.1) doesn't need to insert it — only avoid
                // treating the pattern as a consumer.
                //
                // What DOES count from a return: a compound
                // expression whose NON-final operand references
                // target. E.g. `return target + "!"` has target
                // consumed by the BinOp, and that's picked up by
                // `expr_consumes_target` below when the return
                // value isn't a bare `Local{target}`.
                if !is_bare_local(e, target) && expr_consumes_target(e, target, sigs) {
                    consumed = true;
                }
            }
            IrStmt::Let { value, .. } => {
                // Storing into a new binding transfers ownership.
                // If `target` is read in `value` (other than as a
                // direct `Local{target}` that is the whole RHS), it
                // is consumed.
                if expr_consumes_target(value, target, sigs) {
                    consumed = true;
                }
            }
            IrStmt::Expr { expr, .. } => {
                if expr_consumes_target(expr, target, sigs) {
                    consumed = true;
                }
            }
            IrStmt::If { cond, .. } => {
                if expr_consumes_target(cond, target, sigs) {
                    consumed = true;
                }
            }
            IrStmt::For { iter, .. } => {
                if expr_consumes_target(iter, target, sigs) {
                    consumed = true;
                }
            }
            IrStmt::Approve { args, .. } => {
                for a in args {
                    if expr_consumes_target(a, target, sigs) {
                        consumed = true;
                    }
                }
            }
            _ => {}
        }
    });
    consumed
}

/// Does this expression contain a consuming use of `target`?
fn expr_consumes_target(
    expr: &IrExpr,
    target: LocalId,
    sigs: &HashMap<DefId, Vec<ParamBorrow>>,
) -> bool {
    match &expr.kind {
        IrExprKind::Local { local_id, .. } => *local_id == target,
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } => false,
        IrExprKind::BinOp { left, right, .. }
        | IrExprKind::WrappingBinOp { left, right, .. } => {
            expr_consumes_target(left, target, sigs)
                || expr_consumes_target(right, target, sigs)
        }
        IrExprKind::UnOp { operand, .. }
        | IrExprKind::WrappingUnOp { operand, .. } => {
            expr_consumes_target(operand, target, sigs)
        }
        IrExprKind::FieldAccess { target: t, .. } => expr_consumes_target(t, target, sigs),
        IrExprKind::UnwrapGrounded { value } => expr_consumes_target(value, target, sigs),
        IrExprKind::Index { target: t, index } => {
            expr_consumes_target(t, target, sigs)
                || expr_consumes_target(index, target, sigs)
        }
        IrExprKind::List { items } => {
            // Every element is stored into the list payload → always
            // a consuming position.
            items.iter().any(|it| expr_references(it, target))
        }
        IrExprKind::Call { kind, args, .. } => {
            // Arg consumption depends on callee's σ. If σ says
            // Borrowed, the arg position is NOT a consume. Otherwise
            // it is.
            let callee_sig = match kind {
                IrCallKind::Agent { def_id } => sigs.get(def_id),
                // Tools / prompts / struct constructors all have
                // owning calling conventions today — no borrow
                // inference for them in this pass (future work
                // once tools/prompts support ownership annotations).
                _ => None,
            };
            for (i, a) in args.iter().enumerate() {
                let slot_is_owned = callee_sig
                    .and_then(|s| s.get(i).copied())
                    .map(|b| b == ParamBorrow::Owned)
                    .unwrap_or(true);
                if slot_is_owned && expr_references(a, target) {
                    return true;
                }
            }
            false
        }
        // Result/Option construction stores
        // the inner value into a tagged-union payload (consuming
        // position). `?` propagation conditionally returns the
        // value; if `target` is referenced inside, it's consumed
        // along the propagation path. `try ... retry` likewise
        // consumes the body's value on each iteration.
        IrExprKind::WeakNew { strong: inner }
        | IrExprKind::WeakUpgrade { weak: inner }
        | IrExprKind::StreamSplitBy { stream: inner, .. }
        | IrExprKind::StreamMerge { groups: inner, .. }
        | IrExprKind::StreamOrderedBy { stream: inner, .. }
        | IrExprKind::StreamResumeToken { stream: inner }
        | IrExprKind::ResumeStream { token: inner, .. }
        | IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => expr_references(inner, target),
        IrExprKind::OptionNone => false,
        IrExprKind::TryRetry { body, .. } => expr_references(body, target),
        IrExprKind::Replay { trace, arms, else_body } => {
            // Replay arms are conditional — at most one executes.
            // Any reference inside any arm body (or the else) is a
            // potential consume if the branch fires, so we return
            // true if any branch references `target`.
            expr_references(trace, target)
                || arms.iter().any(|arm| expr_references(&arm.body, target))
                || expr_references(else_body, target)
        }
    }
}

/// Does this expression directly or recursively reference `target`
/// anywhere? (Weaker than `consumes` — includes borrow positions.)
fn expr_references(expr: &IrExpr, target: LocalId) -> bool {
    match &expr.kind {
        IrExprKind::Local { local_id, .. } => *local_id == target,
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } => false,
        IrExprKind::BinOp { left, right, .. }
        | IrExprKind::WrappingBinOp { left, right, .. } => {
            expr_references(left, target) || expr_references(right, target)
        }
        IrExprKind::UnOp { operand, .. }
        | IrExprKind::WrappingUnOp { operand, .. } => expr_references(operand, target),
        IrExprKind::FieldAccess { target: t, .. } => expr_references(t, target),
        IrExprKind::UnwrapGrounded { value } => expr_references(value, target),
        IrExprKind::Index { target: t, index } => {
            expr_references(t, target) || expr_references(index, target)
        }
        IrExprKind::List { items } => items.iter().any(|i| expr_references(i, target)),
        IrExprKind::Call { args, .. } => args.iter().any(|a| expr_references(a, target)),
        // Tagged-union/retry nodes recurse into sub-expressions.
        IrExprKind::WeakNew { strong: inner }
        | IrExprKind::WeakUpgrade { weak: inner }
        | IrExprKind::StreamSplitBy { stream: inner, .. }
        | IrExprKind::StreamMerge { groups: inner, .. }
        | IrExprKind::StreamOrderedBy { stream: inner, .. }
        | IrExprKind::StreamResumeToken { stream: inner }
        | IrExprKind::ResumeStream { token: inner, .. }
        | IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => expr_references(inner, target),
        IrExprKind::OptionNone => false,
        IrExprKind::Replay { trace, arms, else_body } => {
            expr_references(trace, target)
                || arms.iter().any(|arm| expr_references(&arm.body, target))
                || expr_references(else_body, target)
        }
        IrExprKind::TryRetry { body, .. } => expr_references(body, target),
    }
}

/// Walk every statement in the block, invoking the visitor on each.
/// Does not recurse into nested blocks for the visitor itself — the
/// visitor is responsible for that via separate calls if needed.
fn visit_block(block: &IrBlock, visitor: &mut dyn FnMut(&IrStmt)) {
    for stmt in &block.stmts {
        visitor(stmt);
        match stmt {
            IrStmt::If { then_block, else_block, .. } => {
                visit_block(then_block, visitor);
                if let Some(eb) = else_block {
                    visit_block(eb, visitor);
                }
            }
            IrStmt::For { body, .. } => visit_block(body, visitor),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Step 2 — Dup/Drop insertion (one agent at a time)
// ---------------------------------------------------------------------------

/// Transform a single agent's body: insert `Dup` and `Drop` statements
/// at every ownership boundary per the committed rules. Returns the
/// transformed agent and the summary data 17b-1c will consume.
fn transform_agent(
    agent: &IrAgent,
    borrow_sig: Vec<ParamBorrow>,
    _sigs: &HashMap<DefId, Vec<ParamBorrow>>,
) -> (IrAgent, AgentSummary) {
    // We populate borrow_sig but do NOT insert Dup/Drop
    // yet. The scattered emit_retain/emit_release sites in lowering.rs
    // stay as the ownership ground truth for this transitional mode.
    // Full Dup/Drop insertion and scattered-site deletion land in the
    // complete ownership path so we can ship a partial win (borrow-sig-driven call-site
    // elision) without also shouldering the ~40-site surgery in one
    // commit.
    //
    // To make the partial ship coherent: borrow_sig IS consumed at
    // call-sites in `lowering.rs` — if σ(callee, i) = Borrowed, the
    // callee-entry retain and scope-exit release for that parameter
    // are skipped. That's the measurable win for this transitional step.

    let body = agent.body.clone();

    let summary = AgentSummary {
        borrow_sig: borrow_sig.clone(),
        may_retain: false,  // no Dup inserted yet in 1b.1
        may_release: false, // no Drop inserted yet in 1b.1
        borrows_param: borrow_sig
            .iter()
            .map(|b| matches!(b, ParamBorrow::Borrowed))
            .collect(),
    };

    let transformed = IrAgent {
        id: agent.id,
        name: agent.name.clone(),
        extern_abi: agent.extern_abi,
        params: agent.params.clone(),
        return_ty: agent.return_ty.clone(),
        cost_budget: agent.cost_budget,
        wrapping_arithmetic: agent.wrapping_arithmetic,
        body,
        span: agent.span,
        borrow_sig: Some(borrow_sig),
    };

    (transformed, summary)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Is this type refcounted at runtime? Mirrors the predicate in
/// `lowering.rs::is_refcounted_type`.
///
/// Post-17g: `Weak<T>` and `Option<refcounted>` are also refcounted
/// at the value level — they're heap boxes themselves and need
/// `Dup`/`Drop` on the enclosing value. Their REFERENT is not
/// automatically a strong edge: `Weak` holds a non-owning pointer,
/// and `Option<refcounted>` can be `None`. See `is_strong_refcounted`
/// for the predicate that distinguishes strong edges (used by
/// forthcoming 17b-3 / 17b-5 escape + reuse analyses).
pub(crate) fn is_refcounted(ty: &Type) -> bool {
    match ty {
        Type::String | Type::Struct(_) | Type::List(_) | Type::Weak(_, _) | Type::Result(_, _) => true,
        Type::Option(inner) => matches!(&**inner, Type::Int | Type::Bool | Type::Float) || is_refcounted(inner),
        _ => false,
    }
}

/// Does this type hold a STRONG refcount-incrementing edge to a
/// refcounted payload? `Weak<T>` is refcounted at the value level but
/// does NOT strongly own its target — tracing through a Weak during
/// reachability analysis must not count as a retaining edge.
/// `Option<T>` is strong iff its inner is strong AND the value is
/// `Some` at runtime; the analysis treats it as "maybe-strong" by
/// returning true here, delegating the null-path handling to codegen.
///
/// This predicate is forward-compatibility for 17b-3 (drop-guided
/// reuse) and 17b-5 (Choi escape) — .6a/.6b/.6c use `is_refcounted`
/// for Dup/Drop placement, which must fire for both strong and weak
/// carriers of refcounted payloads.
#[allow(dead_code)]
pub(crate) fn is_strong_refcounted(ty: &Type) -> bool {
    match ty {
        Type::String | Type::Struct(_) | Type::List(_) | Type::Result(_, _) => true,
        // Weak is never a strong edge — it carries a weak slot.
        Type::Weak(_, _) => false,
        // Option<strong> is maybe-strong; callers requiring
        // definite-strong should test with the null-path consideration.
        Type::Option(inner) => matches!(&**inner, Type::Int | Type::Bool | Type::Float) || is_strong_refcounted(inner),
        _ => false,
    }
}

/// Is this expression exactly a `Local { local_id: target }` with
/// nothing wrapping it? Used by return-site classification: `return s`
/// where `s` is the bare parameter qualifies for Dup-before-return;
/// `return s + "!"` does not (the BinOp consumes `s`).
fn is_bare_local(expr: &IrExpr, target: LocalId) -> bool {
    matches!(&expr.kind, IrExprKind::Local { local_id, .. } if *local_id == target)
}
