//! IR Dup/Drop insertion driven by the ownership plan.
//!
//! This module is a shadow path. It takes an `IrAgent`, runs the
//! analysis, and returns a NEW agent with `IrStmt::Dup`/`IrStmt::Drop`
//! statements inserted at the precise positions the plan calls for.
//!
//! It is NOT wired into the compilation pipeline yet. The codegen
//! still consumes the un-transformed agent; the scattered
//! `emit_retain` / `emit_release` calls in `lowering.rs` remain the
//! authoritative ownership story until the unified pass took over.
//!
//! Why this ships in this form:
//!   - The ownership plan is a data structure; its correctness can be
//!     unit-tested. But "the plan applied to IR" is a separate
//!     transformation, and *its* correctness can also be unit-tested
//!     in isolation — without having to teach codegen to deduplicate
//!     against the existing emit_retain/release sites.
//!   - The integration step will (a) wire this transformation into the pipeline
//!     and (b) delete the 38 scattered emit_retain/release sites + 4
//!     peepholes. The two changes go together because either alone
//!     would either double-RC (if .6c only inserts) or zero-RC (if
//!     .6c only deletes). They MUST land in the same commit.
//!   - Verifying the insertion logic here means the pipeline wiring becomes a
//!     mechanical edit, not a creative one.
//!
//! ## Algorithm
//!
//! 1. Clone the input agent.
//! 2. Run `analyze_agent` on the clone (or on a borrow of the
//!    original — same result).
//! 3. Group all `dup`/`drop_after` plan entries by their `IrPath`.
//!    Sort each group's insertion list by IR-statement index in
//!    DESCENDING order so insertions don't shift later positions.
//! 4. Walk the IR tree following each path; at each leaf position,
//!    insert the planned `IrStmt::Dup` / `IrStmt::Drop` ops.
//!
//! ## Block-exit drops
//!
//! `OwnershipPlan::drops_at_block_exit` references CFG block IDs that
//! have no IR-tree analog — the entry CFG block is a flattening of
//! the agent's whole body. For .6b we resolve "drop at entry-block
//! exit" as "append a Drop at the END of `agent.body.stmts`, BEFORE
//! any trailing Return". For non-entry CFG blocks (synthetic join
//! blocks etc.) we don't currently emit drops here — the analysis
//! never assigns to those, since liveness only flags block-exit
//! drops for unused parameters of the function (entry block).

use corvid_ast::Span;
use corvid_ir::{IrAgent, IrBlock, IrExpr, IrExprKind, IrStmt};
use corvid_resolve::LocalId;
use std::collections::BTreeMap;

use crate::dataflow::{analyze_agent, BranchDrops, IrNavStep, IrPath, OwnershipPlan};

/// Public entry: clone `agent` and return a new agent with
/// `IrStmt::Dup` and `IrStmt::Drop` inserted per the ownership plan.
///
/// Pure: does not mutate the input. Stable across runs (the plan is
/// deterministic; insertion order is deterministic within a path).
pub fn insert_dup_drop(agent: &IrAgent) -> IrAgent {
    // Diagnostic: when CORVID_DUP_DROP_DRY_RUN=1 is set, the pass
    // runs as a no-op — useful for bisecting whether a parity
    // failure is caused by pass-inserted Dup/Drop ops or by an
    // unguarded scattered emit site in codegen. Silent under
    // normal operation.
    if std::env::var("CORVID_DUP_DROP_DRY_RUN").map(|v| v == "1").unwrap_or(false) {
        return agent.clone();
    }
    let (_cfg, plan) = analyze_agent(agent);
    apply_plan(agent, &plan)
}

/// Apply a pre-computed plan to a clone of `agent`. Split out from
/// `insert_dup_drop` so tests can inject hand-built plans for edge
/// cases the analyzer doesn't generate naturally.
pub fn apply_plan(agent: &IrAgent, plan: &OwnershipPlan) -> IrAgent {
    let mut out = agent.clone();

    // Fresh-LocalId allocator for synthetic-Let hoisting. Scan the
    // agent to find the max LocalId in use across params + all Let /
    // For / Dup / Drop statements, then allocate fresh IDs starting
    // from max+1. Bumping guarantees the hoisted `tmp` never aliases
    // any user-visible local.
    let mut next_local = next_local_id(agent);

    // Group insertions by IR-tree position. Each map is
    //   IrPath (without the trailing Stmt step) → Vec<(stmt_idx, IrStmt)>
    // sorted by stmt_idx DESCENDING so we insert from the back of the
    // block forward, keeping earlier indices stable.
    let mut group: BTreeMap<IrPath, Vec<(usize, IrStmt, InsertionSide)>> = BTreeMap::new();
    // Hoist instructions: for each Return whose expr reads any of
    // the drop-after locals, replace the expr with a bare Local ref
    // to a fresh tmp and insert a preceding Let + Drop sequence.
    // Separated from `group` because the hoist requires mutating the
    // Return statement itself, not just inserting siblings.
    let mut hoists: Vec<HoistPlan> = Vec::new();

    for (pp, locals) in &plan.dups {
        if let Some(path) = plan.ir_paths.get(pp) {
            let (parent, idx) = split_path(path);
            for &l in locals {
                group.entry(parent.clone()).or_default().push((
                    idx,
                    IrStmt::Dup { local_id: l, span: zero_span() },
                    InsertionSide::Before,
                ));
            }
        }
    }
    for (pp, locals) in &plan.drops_after {
        if let Some(path) = plan.ir_paths.get(pp) {
            let (parent, idx) = split_path(path);
            // A drop_after on a Return statement is unreachable if we
            // place it after the Return (never executes) and UAF-
            // hazardous if we place it before (return's expr still
            // reads the local). Two cases:
            //
            //   - If the Return's expr does NOT reference any of the
            //     locals being dropped, drop-before-Return is safe.
            //     Promote to Before.
            //   - If it DOES reference any, hoist the expr into a
            //     synthetic `Let tmp = <expr>; Drop <locals>; Return tmp`.
            //     The Drops then land after the expr has evaluated
            //     but before the return instruction fires.
            let target_block = find_block(&agent.body, &parent);
            let return_expr = target_block
                .and_then(|b| b.stmts.get(idx))
                .and_then(|s| match s {
                    IrStmt::Return { value: Some(e), .. } => Some(e.clone()),
                    _ => None,
                });

            if let Some(expr) = return_expr {
                let needs_hoist = locals.iter().any(|&l| expr_reads_local(&expr, l));
                if needs_hoist {
                    let tmp_id = LocalId(next_local);
                    next_local += 1;
                    hoists.push(HoistPlan {
                        parent: parent.clone(),
                        return_idx: idx,
                        tmp_id,
                        drop_locals: locals.iter().copied().collect(),
                    });
                    continue;
                }
                // Non-reading Return: safe to drop-before.
                for &l in locals {
                    group.entry(parent.clone()).or_default().push((
                        idx,
                        IrStmt::Drop { local_id: l, span: zero_span() },
                        InsertionSide::Before,
                    ));
                }
                continue;
            }
            // Not a Return with a value-expr: normal drop-after.
            for &l in locals {
                group.entry(parent.clone()).or_default().push((
                    idx,
                    IrStmt::Drop { local_id: l, span: zero_span() },
                    InsertionSide::After,
                ));
            }
        }
    }

    // Sort each group's insertions: by (idx desc, side desc-so-After-first-then-Before).
    // Inserting from the back means earlier positions don't shift. At
    // the SAME idx, we want After insertions to land at idx+1 BEFORE
    // any Before insertions land at idx; otherwise the After's "idx+1"
    // becomes wrong once a Before shifts everything by one.
    // Concrete order:
    //   1. Process highest idx first.
    //   2. At equal idx, do `After` before `Before` (After inserts at
    //      idx+1; Before inserts at idx; processing After first leaves
    //      idx untouched for the subsequent Before insertion).
    for inserts in group.values_mut() {
        inserts.sort_by(|a, b| {
            b.0.cmp(&a.0).then_with(|| match (a.2, b.2) {
                (InsertionSide::After, InsertionSide::Before) => std::cmp::Ordering::Less,
                (InsertionSide::Before, InsertionSide::After) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            })
        });
    }

    for (parent, inserts) in group {
        let target = navigate_mut(&mut out.body, &parent);
        for (idx, stmt, side) in inserts {
            let insert_at = match side {
                InsertionSide::Before => idx,
                InsertionSide::After => idx + 1,
            };
            // Bound-check: if idx is out of range (shouldn't happen
            // for a well-formed plan), skip rather than panic.
            if insert_at <= target.stmts.len() {
                target.stmts.insert(insert_at, stmt);
            }
        }
    }

    // Apply hoists AFTER the plain insertions. Hoists must run last
    // because they depend on the Return statement's index being
    // stable; if another drop_after widened the same block between
    // group-sort and hoist-apply, the Return's index would shift.
    // We sort hoists largest-idx-first so earlier hoists don't
    // invalidate later ones in the same block.
    hoists.sort_by(|a, b| b.return_idx.cmp(&a.return_idx));
    for hoist in hoists {
        apply_hoist(&mut out.body, &hoist);
    }

    // Branch-specialized drops. For each If statement
    // that analysis identified as needing per-branch drops,
    // inject `IrStmt::Drop` ops at the tail of the appropriate
    // branch. Deepest-nested Ifs processed first so earlier
    // injections don't shift outer If indices.
    //
    // Keyed by IrPath ordered by length descending so a branch drop
    // inside a nested If lands before its enclosing If is mutated.
    let mut branch_drops: Vec<_> = plan.branch_drops.iter().collect();
    branch_drops.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    for (if_path, drops) in branch_drops {
        apply_branch_drops(&mut out.body, if_path, drops);
    }

    // Block-exit drops: only handle the entry block (block 0 in the
    // CFG). Append `IrStmt::Drop` to the end of the agent body, but
    // place it BEFORE any trailing Return so the drop actually
    // executes.
    if let Some(locals) = plan.drops_at_block_exit.get(&0) {
        let stmts = &mut out.body.stmts;
        let ret_pos = stmts.iter().rposition(|s| matches!(s, IrStmt::Return { .. }));
        let insert_at = ret_pos.unwrap_or(stmts.len());
        // Iterate in stable order (BTreeSet) for deterministic output.
        for &l in locals {
            stmts.insert(
                insert_at,
                IrStmt::Drop { local_id: l, span: zero_span() },
            );
        }
    }

    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InsertionSide {
    Before,
    After,
}

/// Recipe for hoisting a Return's expression into a synthetic Let
/// so drops can fire between expr eval and the actual return.
///
/// Before:                     After:
///   Return { expr: E }          Let tmp = E
///                               Drop drop_locals...
///                               Return { expr: Local(tmp) }
struct HoistPlan {
    /// Parent path leading to the block containing the Return.
    parent: IrPath,
    /// Index of the Return statement within that block.
    return_idx: usize,
    /// Fresh LocalId allocated for the synthetic tmp binding.
    tmp_id: LocalId,
    /// Locals to Drop between the Let and the Return.
    drop_locals: Vec<LocalId>,
}

/// Scan the agent's body to find the highest LocalId already in use,
/// plus parameter IDs. Returns `max + 1` as the starting point for
/// synthetic-Let fresh allocations.
///
/// Must cover every IR shape that can bind or reference a LocalId:
/// params, Let.local_id, For.var_local, Dup.local_id, Drop.local_id,
/// and any IrExprKind::Local's local_id inside nested expressions.
fn next_local_id(agent: &IrAgent) -> u32 {
    let mut max_id: u32 = 0;
    for p in &agent.params {
        if p.local_id.0 > max_id { max_id = p.local_id.0; }
    }
    scan_block(&agent.body, &mut max_id);
    max_id + 1
}

fn scan_block(block: &IrBlock, max_id: &mut u32) {
    for stmt in &block.stmts {
        scan_stmt(stmt, max_id);
    }
}

fn scan_stmt(stmt: &IrStmt, max_id: &mut u32) {
    match stmt {
        IrStmt::Let { local_id, value, .. } => {
            if local_id.0 > *max_id { *max_id = local_id.0; }
            scan_expr(value, max_id);
        }
        IrStmt::Return { value: Some(e), .. } => scan_expr(e, max_id),
        IrStmt::Return { value: None, .. } => {}
        IrStmt::Yield { value, .. } => scan_expr(value, max_id),
        IrStmt::If { cond, then_block, else_block, .. } => {
            scan_expr(cond, max_id);
            scan_block(then_block, max_id);
            if let Some(eb) = else_block { scan_block(eb, max_id); }
        }
        IrStmt::For { var_local, iter, body, .. } => {
            if var_local.0 > *max_id { *max_id = var_local.0; }
            scan_expr(iter, max_id);
            scan_block(body, max_id);
        }
        IrStmt::Expr { expr, .. } => scan_expr(expr, max_id),
        IrStmt::Approve { args, .. } => {
            for a in args { scan_expr(a, max_id); }
        }
        IrStmt::Dup { local_id, .. } | IrStmt::Drop { local_id, .. } => {
            if local_id.0 > *max_id { *max_id = local_id.0; }
        }
        IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => {}
    }
}

fn scan_expr(expr: &IrExpr, max_id: &mut u32) {
    match &expr.kind {
        IrExprKind::Local { local_id, .. } => {
            if local_id.0 > *max_id { *max_id = local_id.0; }
        }
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } | IrExprKind::OptionNone => {}
        IrExprKind::FieldAccess { target, .. } => scan_expr(target, max_id),
        IrExprKind::UnwrapGrounded { value } => scan_expr(value, max_id),
        IrExprKind::Index { target, index } => {
            scan_expr(target, max_id);
            scan_expr(index, max_id);
        }
        IrExprKind::BinOp { left, right, .. }
        | IrExprKind::WrappingBinOp { left, right, .. } => {
            scan_expr(left, max_id);
            scan_expr(right, max_id);
        }
        IrExprKind::UnOp { operand, .. }
        | IrExprKind::WrappingUnOp { operand, .. } => scan_expr(operand, max_id),
        IrExprKind::Call { args, .. } => {
            for a in args { scan_expr(a, max_id); }
        }
        IrExprKind::List { items } => {
            for item in items { scan_expr(item, max_id); }
        }
        IrExprKind::WeakNew { strong } => scan_expr(strong, max_id),
        IrExprKind::WeakUpgrade { weak } => scan_expr(weak, max_id),
        IrExprKind::StreamSplitBy { stream, .. } => scan_expr(stream, max_id),
        IrExprKind::StreamMerge { groups, .. } => scan_expr(groups, max_id),
        IrExprKind::StreamOrderedBy { stream, .. } => scan_expr(stream, max_id),
        IrExprKind::StreamResumeToken { stream } => scan_expr(stream, max_id),
        IrExprKind::ResumeStream { token, .. } => scan_expr(token, max_id),
        IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => scan_expr(inner, max_id),
        IrExprKind::TryRetry { body, .. } => scan_expr(body, max_id),
        IrExprKind::Replay { trace, arms, else_body } => {
            scan_expr(trace, max_id);
            for arm in arms {
                if let Some(capture) = &arm.capture {
                    if capture.local_id.0 > *max_id { *max_id = capture.local_id.0; }
                }
                if let corvid_ir::IrReplayPattern::Tool {
                    arg: corvid_ir::IrReplayToolArgPattern::Capture(capture),
                    ..
                } = &arm.pattern
                {
                    if capture.local_id.0 > *max_id { *max_id = capture.local_id.0; }
                }
                scan_expr(&arm.body, max_id);
            }
            scan_expr(else_body, max_id);
        }
    }
}

/// Walk `expr` and return true if it contains any read of `target`.
/// Used to decide whether a Return's expr forces a synthetic-Let hoist.
fn expr_reads_local(expr: &IrExpr, target: LocalId) -> bool {
    match &expr.kind {
        IrExprKind::Local { local_id, .. } => *local_id == target,
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } | IrExprKind::OptionNone => false,
        IrExprKind::FieldAccess { target: t, .. } => expr_reads_local(t, target),
        IrExprKind::UnwrapGrounded { value } => expr_reads_local(value, target),
        IrExprKind::Index { target: t, index } => {
            expr_reads_local(t, target) || expr_reads_local(index, target)
        }
        IrExprKind::BinOp { left, right, .. }
        | IrExprKind::WrappingBinOp { left, right, .. } => {
            expr_reads_local(left, target) || expr_reads_local(right, target)
        }
        IrExprKind::UnOp { operand, .. }
        | IrExprKind::WrappingUnOp { operand, .. } => expr_reads_local(operand, target),
        IrExprKind::Call { args, .. } => args.iter().any(|a| expr_reads_local(a, target)),
        IrExprKind::List { items } => items.iter().any(|it| expr_reads_local(it, target)),
        IrExprKind::WeakNew { strong } => expr_reads_local(strong, target),
        IrExprKind::WeakUpgrade { weak } => expr_reads_local(weak, target),
        IrExprKind::StreamSplitBy { stream, .. } => expr_reads_local(stream, target),
        IrExprKind::StreamMerge { groups, .. } => expr_reads_local(groups, target),
        IrExprKind::StreamOrderedBy { stream, .. } => expr_reads_local(stream, target),
        IrExprKind::StreamResumeToken { stream } => expr_reads_local(stream, target),
        IrExprKind::ResumeStream { token, .. } => expr_reads_local(token, target),
        IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => expr_reads_local(inner, target),
        IrExprKind::TryRetry { body, .. } => expr_reads_local(body, target),
        IrExprKind::Replay { trace, arms, else_body } => {
            expr_reads_local(trace, target)
                || arms.iter().any(|arm| expr_reads_local(&arm.body, target))
                || expr_reads_local(else_body, target)
        }
    }
}

/// Execute one HoistPlan against the IR tree. Replaces `Return { E }`
/// with `[Let tmp = E, Drop locals..., Return { Local(tmp) }]`.
fn apply_hoist(root: &mut IrBlock, hoist: &HoistPlan) {
    let block = navigate_mut(root, &hoist.parent);
    let ret_idx = hoist.return_idx;
    // Bound-check: if the block was rewritten between plan construction
    // and hoist application, bail out rather than panic.
    if ret_idx >= block.stmts.len() {
        return;
    }
    let Some(orig_expr) = (match &block.stmts[ret_idx] {
        IrStmt::Return { value: Some(e), .. } => Some(e.clone()),
        _ => None,
    }) else {
        return;
    };
    let return_span = match &block.stmts[ret_idx] {
        IrStmt::Return { span, .. } => *span,
        _ => zero_span(),
    };
    let expr_ty = orig_expr.ty.clone();
    let expr_span = orig_expr.span;

    // Build the replacement sequence.
    //   Let tmp = <orig>
    //   Drop d0; Drop d1; ...
    //   Return Local(tmp)
    let let_stmt = IrStmt::Let {
        local_id: hoist.tmp_id,
        name: "__corvid_ret_tmp".to_string(),
        ty: expr_ty.clone(),
        value: orig_expr,
        span: return_span,
    };
    let mut drop_stmts: Vec<IrStmt> = hoist
        .drop_locals
        .iter()
        .map(|&l| IrStmt::Drop { local_id: l, span: zero_span() })
        .collect();
    let new_return = IrStmt::Return {
        value: Some(IrExpr {
            kind: IrExprKind::Local {
                local_id: hoist.tmp_id,
                name: "__corvid_ret_tmp".to_string(),
            },
            ty: expr_ty,
            span: expr_span,
        }),
        span: return_span,
    };

    // Splice: replace block.stmts[ret_idx] with [let, drops..., return].
    block.stmts.remove(ret_idx);
    let mut insert_at = ret_idx;
    block.stmts.insert(insert_at, let_stmt);
    insert_at += 1;
    for drop_stmt in drop_stmts.drain(..) {
        block.stmts.insert(insert_at, drop_stmt);
        insert_at += 1;
    }
    block.stmts.insert(insert_at, new_return);
}

/// Split a navigation path into (parent_path, final_stmt_idx).
fn split_path(path: &IrPath) -> (IrPath, usize) {
    let mut parent: IrPath = path.iter().cloned().collect();
    let last = parent.pop().expect("IrPath always ends in Stmt(_)");
    let idx = match last {
        IrNavStep::Stmt(i) => i,
        _ => panic!("malformed IrPath — last step must be Stmt"),
    };
    (parent, idx)
}

/// Navigate from the root `IrBlock` down through nested If/For
/// children to the block containing the target statement. Mutable
/// reference; caller mutates `target.stmts`.
/// Read-only analog of `navigate_mut`. Returns None if the path
/// doesn't match the current IR shape (e.g., caller built the path
/// from a stale CFG). Callers treat None as "can't determine" and
/// fall back to the default insertion side.
fn find_block<'a>(root: &'a IrBlock, parent: &IrPath) -> Option<&'a IrBlock> {
    let mut current: &'a IrBlock = root;
    let mut i = 0;
    while i < parent.len() {
        let stmt_step = &parent[i];
        let descend_step = parent.get(i + 1)?;
        let stmt_idx = match stmt_step {
            IrNavStep::Stmt(s) => *s,
            _ => return None,
        };
        let stmt = current.stmts.get(stmt_idx)?;
        current = match (stmt, descend_step) {
            (IrStmt::If { then_block, .. }, IrNavStep::IfThen) => then_block,
            (IrStmt::If { else_block: Some(eb), .. }, IrNavStep::IfElse) => eb,
            (IrStmt::For { body, .. }, IrNavStep::ForBody) => body,
            _ => return None,
        };
        i += 2;
    }
    Some(current)
}

fn navigate_mut<'a>(root: &'a mut IrBlock, parent: &IrPath) -> &'a mut IrBlock {
    let mut current: &'a mut IrBlock = root;
    let mut i = 0;
    while i < parent.len() {
        // Each "descent" consumes a Stmt step (telling which child of
        // the current block to enter) followed by an If/For step
        // (which child block of that statement).
        let stmt_step = &parent[i];
        let descend_step = parent.get(i + 1);
        let stmt_idx = match stmt_step {
            IrNavStep::Stmt(s) => *s,
            _ => panic!("malformed IrPath — descent without preceding Stmt"),
        };
        let descend = match descend_step {
            Some(s) => s.clone(),
            None => panic!("malformed IrPath — Stmt with no descent step"),
        };
        let stmt = &mut current.stmts[stmt_idx];
        current = match (stmt, descend) {
            (IrStmt::If { then_block, .. }, IrNavStep::IfThen) => then_block,
            (IrStmt::If { else_block: Some(eb), .. }, IrNavStep::IfElse) => eb,
            (IrStmt::For { body, .. }, IrNavStep::ForBody) => body,
            _ => panic!("IrPath descent doesn't match IR shape"),
        };
        i += 2;
    }
    current
}

fn zero_span() -> Span {
    Span { start: 0, end: 0 }
}

/// Apply per-branch drops to an If statement.
///
/// `if_path` points to the If statement inside `root`. `drops` lists
/// locals that die on each branch edge per the dataflow analysis.
///
/// For the then-edge: append `Drop` ops to the tail of
/// `then_block`. Sorting BranchDrops via BTreeSet keeps the order
/// deterministic.
///
/// For the fallthrough-edge: append to the tail of `else_block` if
/// it exists; otherwise synthesize an else_block containing only
/// the Drop ops.
fn apply_branch_drops(root: &mut IrBlock, if_path: &IrPath, drops: &BranchDrops) {
    // Navigate to the parent block containing the If statement, then
    // fetch the specific If via its final Stmt index.
    let (parent, idx) = split_path(if_path);
    let parent_block = navigate_mut(root, &parent);
    if idx >= parent_block.stmts.len() {
        return;
    }
    let if_stmt = &mut parent_block.stmts[idx];
    let (then_block, else_block, if_span) = match if_stmt {
        IrStmt::If { then_block, else_block, span, .. } => {
            (then_block, else_block, *span)
        }
        _ => return, // stale path
    };

    // Then-branch: append Drops to end of then_block, or before
    // any trailing Return so they actually execute.
    insert_drops_at_block_tail(then_block, &drops.then_drops);

    // Fallthrough/else-branch: append to existing else_block or
    // synthesize a new one. Note: synthesizing an else_block is a
    // semantic change — it means control now passes through a new
    // explicit block on the fall-through path. Since the new block
    // contains only Drops, all-primitive pure computation outside
    // the If continues to work; the only observable difference is
    // the Drops fire where they previously didn't.
    if !drops.fallthrough_drops.is_empty() {
        match else_block {
            Some(eb) => insert_drops_at_block_tail(eb, &drops.fallthrough_drops),
            None => {
                let drop_stmts: Vec<IrStmt> = drops
                    .fallthrough_drops
                    .iter()
                    .map(|&l| IrStmt::Drop { local_id: l, span: zero_span() })
                    .collect();
                *else_block = Some(IrBlock {
                    stmts: drop_stmts,
                    span: if_span,
                });
            }
        }
    }
}

/// Append Drop ops at the tail of `block`, but BEFORE any trailing
/// Return — Drops after a Return are unreachable at runtime. Locals
/// iterated in BTreeSet order for deterministic output.
fn insert_drops_at_block_tail(block: &mut IrBlock, locals: &std::collections::BTreeSet<corvid_resolve::LocalId>) {
    if locals.is_empty() {
        return;
    }
    let ret_pos = block.stmts.iter().rposition(|s| matches!(s, IrStmt::Return { .. }));
    let insert_at = ret_pos.unwrap_or(block.stmts.len());
    let mut at = insert_at;
    for &l in locals {
        block.stmts.insert(at, IrStmt::Drop {
            local_id: l,
            span: zero_span(),
        });
        at += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::BinaryOp;
    use corvid_ir::{IrAgent, IrBlock, IrExpr, IrExprKind, IrParam, IrStmt};
    use corvid_resolve::{DefId, LocalId};
    use corvid_types::Type;

    fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    fn local_expr(id: u32, ty: Type) -> IrExpr {
        IrExpr {
            kind: IrExprKind::Local { local_id: LocalId(id), name: format!("l{id}") },
            ty,
            span: span(),
        }
    }

    fn make_agent(params: Vec<(u32, Type)>, body: Vec<IrStmt>, ret: Type) -> IrAgent {
        IrAgent {
            id: DefId(0),
            name: "test".into(),
            extern_abi: None,
            params: params
                .into_iter()
                .map(|(id, ty)| IrParam {
                    name: format!("p{id}"),
                    local_id: LocalId(id),
                    ty,
                    span: span(),
                })
                .collect(),
            return_ty: ret,
            cost_budget: None,
            wrapping_arithmetic: false,
            body: IrBlock { stmts: body, span: span() },
            span: span(),
            borrow_sig: None,
        }
    }

    /// `let t = s + s; return t` → first read of `s` in the let needs
    /// a Dup. Result IR: [Dup s, Let t = s+s, Return t].
    #[test]
    fn dup_inserted_before_non_last_use() {
        let agent = make_agent(
            vec![(0, Type::String)],
            vec![
                IrStmt::Let {
                    local_id: LocalId(1),
                    name: "t".into(),
                    ty: Type::String,
                    value: IrExpr {
                        kind: IrExprKind::BinOp {
                            op: BinaryOp::Add,
                            left: Box::new(local_expr(0, Type::String)),
                            right: Box::new(local_expr(0, Type::String)),
                        },
                        ty: Type::String,
                        span: span(),
                    },
                    span: span(),
                },
                IrStmt::Return {
                    value: Some(local_expr(1, Type::String)),
                    span: span(),
                },
            ],
            Type::String,
        );
        let out = insert_dup_drop(&agent);
        assert_eq!(out.body.stmts.len(), 3, "Dup should have been inserted before the Let");
        assert!(matches!(out.body.stmts[0], IrStmt::Dup { local_id: LocalId(0), .. }));
        assert!(matches!(out.body.stmts[1], IrStmt::Let { .. }));
        assert!(matches!(out.body.stmts[2], IrStmt::Return { .. }));
    }

    /// Unused String param → Drop appears at the end of the body,
    /// BEFORE the Return.
    #[test]
    fn unused_param_drops_before_return() {
        let agent = make_agent(
            vec![(0, Type::String)],
            vec![IrStmt::Return {
                value: Some(IrExpr {
                    kind: IrExprKind::Literal(corvid_ir::IrLiteral::Int(0)),
                    ty: Type::Int,
                    span: span(),
                }),
                span: span(),
            }],
            Type::Int,
        );
        let out = insert_dup_drop(&agent);
        assert_eq!(out.body.stmts.len(), 2);
        assert!(
            matches!(out.body.stmts[0], IrStmt::Drop { local_id: LocalId(0), .. }),
            "unused param drop comes before return"
        );
        assert!(matches!(out.body.stmts[1], IrStmt::Return { .. }));
    }

    /// `let t = s; return 0` (t never used) → Drop t after the Let,
    /// before the Return.
    #[test]
    fn defined_unused_drops_after_let() {
        let agent = make_agent(
            vec![(0, Type::String)],
            vec![
                IrStmt::Let {
                    local_id: LocalId(1),
                    name: "t".into(),
                    ty: Type::String,
                    value: local_expr(0, Type::String),
                    span: span(),
                },
                IrStmt::Return {
                    value: Some(IrExpr {
                        kind: IrExprKind::Literal(corvid_ir::IrLiteral::Int(0)),
                        ty: Type::Int,
                        span: span(),
                    }),
                    span: span(),
                },
            ],
            Type::Int,
        );
        let out = insert_dup_drop(&agent);
        // Expected order: [Let t, Drop t, Return 0]. Param 0 (s) is
        // consumed by the Let (last use of s), so no extra Dup needed.
        assert_eq!(out.body.stmts.len(), 3);
        assert!(matches!(out.body.stmts[0], IrStmt::Let { .. }));
        assert!(matches!(out.body.stmts[1], IrStmt::Drop { local_id: LocalId(1), .. }));
        assert!(matches!(out.body.stmts[2], IrStmt::Return { .. }));
    }

    /// `return s` where s is the bare last use → no insertions.
    #[test]
    fn no_op_for_already_optimal() {
        let agent = make_agent(
            vec![(0, Type::String)],
            vec![IrStmt::Return {
                value: Some(local_expr(0, Type::String)),
                span: span(),
            }],
            Type::String,
        );
        let out = insert_dup_drop(&agent);
        assert_eq!(out.body.stmts.len(), 1, "no Dup or Drop should be inserted");
    }

    /// Deterministic output: two runs on equal IR produce equal IR.
    #[test]
    fn determinism_two_runs() {
        let mk = || {
            make_agent(
                vec![(0, Type::String)],
                vec![
                    IrStmt::Let {
                        local_id: LocalId(1),
                        name: "t".into(),
                        ty: Type::String,
                        value: local_expr(0, Type::String),
                        span: span(),
                    },
                    IrStmt::Return {
                        value: Some(local_expr(1, Type::String)),
                        span: span(),
                    },
                ],
                Type::String,
            )
        };
        let a = insert_dup_drop(&mk());
        let b = insert_dup_drop(&mk());
        assert_eq!(a.body.stmts.len(), b.body.stmts.len());
        for (sa, sb) in a.body.stmts.iter().zip(b.body.stmts.iter()) {
            // Compare structural variant — Spans we use are zero so
            // matching the tag is enough for determinism evidence.
            assert_eq!(
                std::mem::discriminant(sa),
                std::mem::discriminant(sb),
                "two runs produced different IR shape"
            );
        }
    }

    /// Insertion inside an `if` branch: `if cond: let t = s + s` —
    /// the Dup must land inside the then-block, not at the top level.
    #[test]
    fn dup_inside_then_branch() {
        let agent = make_agent(
            vec![(0, Type::String), (1, Type::Bool)],
            vec![
                IrStmt::If {
                    cond: local_expr(1, Type::Bool),
                    then_block: IrBlock {
                        stmts: vec![IrStmt::Let {
                            local_id: LocalId(2),
                            name: "t".into(),
                            ty: Type::String,
                            value: IrExpr {
                                kind: IrExprKind::BinOp {
                                    op: BinaryOp::Add,
                                    left: Box::new(local_expr(0, Type::String)),
                                    right: Box::new(local_expr(0, Type::String)),
                                },
                                ty: Type::String,
                                span: span(),
                            },
                            span: span(),
                        }],
                        span: span(),
                    },
                    else_block: None,
                    span: span(),
                },
                IrStmt::Return {
                    value: Some(IrExpr {
                        kind: IrExprKind::Literal(corvid_ir::IrLiteral::Int(0)),
                        ty: Type::Int,
                        span: span(),
                    }),
                    span: span(),
                },
            ],
            Type::Int,
        );
        let out = insert_dup_drop(&agent);
        // Top level should still be just [If, Return] — Dup is inside.
        assert!(matches!(out.body.stmts[0], IrStmt::If { .. }));
        if let IrStmt::If { then_block, .. } = &out.body.stmts[0] {
            assert!(
                then_block.stmts.iter().any(|s| matches!(s, IrStmt::Dup { local_id: LocalId(0), .. })),
                "Dup of param 0 should land inside the then-block"
            );
        }
    }
}
