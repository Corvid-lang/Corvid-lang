//! Slice 17b-1b.6b — IR Dup/Drop insertion driven by the .6a plan.
//!
//! This module is a SHADOW PATH. It takes an `IrAgent`, runs the .6a
//! analysis, and returns a NEW agent with `IrStmt::Dup`/`IrStmt::Drop`
//! statements inserted at the precise positions the plan calls for.
//!
//! It is NOT wired into the compilation pipeline yet. The codegen
//! still consumes the un-transformed agent; the scattered
//! `emit_retain` / `emit_release` calls in `lowering.rs` remain the
//! authoritative ownership story until slice .6c.
//!
//! Why ship .6b in this form:
//!   - The plan from .6a is a data structure; its correctness can be
//!     unit-tested. But "the plan applied to IR" is a separate
//!     transformation, and *its* correctness can also be unit-tested
//!     in isolation — without having to teach codegen to deduplicate
//!     against the existing emit_retain/release sites.
//!   - Slice .6c will (a) wire this transformation into the pipeline
//!     and (b) delete the 38 scattered emit_retain/release sites + 4
//!     peepholes. The two changes go together because either alone
//!     would either double-RC (if .6c only inserts) or zero-RC (if
//!     .6c only deletes). They MUST land in the same commit.
//!   - Verifying the insertion logic in .6b means .6c becomes a
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
use corvid_ir::{IrAgent, IrBlock, IrStmt};
use corvid_resolve::LocalId;
use std::collections::BTreeMap;

use crate::dataflow::{analyze_agent, IrNavStep, IrPath, OwnershipPlan, ProgramPoint};

/// Public entry: clone `agent` and return a new agent with
/// `IrStmt::Dup` and `IrStmt::Drop` inserted per the .6a plan.
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

    // Group insertions by IR-tree position. Each map is
    //   IrPath (without the trailing Stmt step) → Vec<(stmt_idx, IrStmt)>
    // sorted by stmt_idx DESCENDING so we insert from the back of the
    // block forward, keeping earlier indices stable.
    let mut group: BTreeMap<IrPath, Vec<(usize, IrStmt, InsertionSide)>> = BTreeMap::new();

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
            // A drop_after on a Return statement is unreachable at
            // runtime — the return exits before the drop can fire.
            // Promote to Before so it runs just before the exit.
            // Check the target block's statement at this index.
            let target_block = find_block(&agent.body, &parent);
            let is_return = target_block
                .and_then(|b| b.stmts.get(idx))
                .map(|s| matches!(s, IrStmt::Return { .. }))
                .unwrap_or(false);
            let side = if is_return {
                InsertionSide::Before
            } else {
                InsertionSide::After
            };
            for &l in locals {
                group.entry(parent.clone()).or_default().push((
                    idx,
                    IrStmt::Drop { local_id: l, span: zero_span() },
                    side,
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
