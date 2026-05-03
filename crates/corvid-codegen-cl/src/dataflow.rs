//! CFG + liveness + ownership dataflow analysis.
//!
//! READ-ONLY in this commit. The analysis runs over an `IrAgent`,
//! computes a per-local ownership plan (where to insert `Dup`, where
//! to insert `Drop`), and returns it as a structured value. It does
//! NOT mutate the IR or change codegen. The ownership rewriter consumes
//! the plan to actually insert `IrStmt::Dup` / `IrStmt::Drop`.
//! The lowering cleanup then deletes the scattered
//! `emit_retain` / `emit_release` sites in `lowering.rs` plus the
//! four ownership peepholes.
//!
//! Design choices:
//!   - split the ownership rollout into analysis, insertion, and
//!     lowering cleanup so each step stays reviewable
//!   - Dataflow precision (not linear scan; not full Perceus).
//!   - The GC verifier (`CORVID_GC_VERIFY=abort`) audits the final
//!     runtime contract.
//!
//! ## Algorithm
//!
//! Step 1 — Linearize the IR's tree of nested blocks (`If`, `For`)
//! into a CFG where every `CfgBlock` is a basic block (single entry,
//! no branches except at the end). Successors are stored explicitly.
//!
//! Step 2 — Walk every `IrStmt` in source order, recording every
//! refcounted-local *use* with a coordinate `(BlockId, StmtPos)` and
//! a classification (`ReadKind::Owned` if the surrounding expression
//! consumes the value, `ReadKind::Borrowed` if it only inspects).
//!
//! Step 3 — Liveness: backward dataflow on the CFG. A local is "live
//! out" of a point P if any successor path uses it before redefining.
//! Last-use of a local at P = used at P AND not live out of P.
//!
//! Step 4 — Build the `OwnershipPlan`:
//!   - For each non-last consuming use, schedule a `Dup` immediately
//!     before that use. Reason: the use will consume the +1, but the
//!     local needs to remain alive for the next use, so we duplicate.
//!   - For each last consuming use, no Dup needed (the use takes the
//!     last +1 and closes the local's lifetime).
//!   - For each local with at least one borrowed-only use and never
//!     consumed: schedule a `Drop` after the local's last use, OR at
//!     scope exit if the local never reaches a use after definition
//!     (purely held).
//!   - For locals defined but never used: schedule `Drop` immediately
//!     after the defining `Let`.
//!
//! ## Non-goals
//!
//! - Does not insert `Dup`/`Drop` into the IR.
//! - Does not change codegen.
//! - Does not handle `Weak<T>` ownership.
//! - Does not perform advanced RC optimizations.

use corvid_ir::IrAgent;
use corvid_resolve::LocalId;
use corvid_types::Type;
use std::collections::BTreeMap;

mod cfg;
mod liveness;
mod ownership_plan;

use cfg::build_cfg;
pub use cfg::{BlockId, Cfg, CfgBlock, CfgStmt, LocalRead, ProgramPoint, ReadKind, StmtPos};
use liveness::compute_liveness;
pub use liveness::Liveness;
use ownership_plan::{build_branch_drops, build_plan};
pub use ownership_plan::{BranchDrops, IrNavStep, IrPath, OwnershipPlan};

use crate::ownership::is_refcounted;

// ---------------------------------------------------------------------------
// Public entry
// ---------------------------------------------------------------------------

/// Run the .6a analysis on a single agent. Returns the CFG + the
/// ownership plan. Pure: does not mutate `agent`.
pub fn analyze_agent(agent: &IrAgent) -> (Cfg, OwnershipPlan) {
    let (cfg, _liveness, plan) = analyze_agent_full(agent);
    (cfg, plan)
}

/// Analysis entry that ALSO returns the `Liveness` result used
/// during plan construction. Downstream passes (17b-6 effect-row-
/// directed RC, 17b-7 latency-aware RC) consume this rather than
/// recomputing. Expected use pattern:
///
/// ```ignore
/// let (cfg, liveness, plan) = analyze_agent_full(&agent);
/// for block_id in 0..liveness.block_count() {
///     let live_out = liveness.live_out_at_block(block_id);
///     // inspect boundary-live refcounted locals...
/// }
/// ```
pub fn analyze_agent_full(agent: &IrAgent) -> (Cfg, Liveness, OwnershipPlan) {
    let built = build_cfg(agent);
    let cfg = built.cfg;
    let ir_paths = built.ir_paths;
    let if_cfg_coords = built.if_cfg_coords;

    // Parameters are defined at function entry. Refcounted parameters
    // start owned (for now — borrow-sig-driven parameter elision is
    // already handled at codegen by the existing borrow_sig consumer
    // and survives this pass unchanged).
    let mut param_locals: BTreeMap<LocalId, Type> = BTreeMap::new();
    for p in &agent.params {
        if is_refcounted(&p.ty) {
            param_locals.insert(p.local_id, p.ty.clone());
        }
    }

    let liveness = compute_liveness(&cfg, &param_locals);
    let mut plan = build_plan(&cfg, &liveness, &param_locals);
    plan.ir_paths = ir_paths;
    plan.branch_drops = build_branch_drops(&cfg, &liveness, &if_cfg_coords, &param_locals);
    (cfg, liveness, plan)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::{BinaryOp, Span};
    use corvid_ir::{IrAgent, IrBlock, IrExpr, IrExprKind, IrParam, IrStmt};
    use corvid_resolve::{DefId, LocalId};
    use corvid_types::Type;

    fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    fn local_expr(id: u32, ty: Type) -> IrExpr {
        IrExpr {
            kind: IrExprKind::Local {
                local_id: LocalId(id),
                name: format!("l{id}"),
            },
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
            body: IrBlock {
                stmts: body,
                span: span(),
            },
            span: span(),
            borrow_sig: None,
        }
    }

    /// A single String parameter, returned bare. The use is the LAST
    /// consuming use → no Dup. No drops.
    #[test]
    fn return_bare_param_no_dup() {
        let agent = make_agent(
            vec![(0, Type::String)],
            vec![IrStmt::Return {
                value: Some(local_expr(0, Type::String)),
                span: span(),
            }],
            Type::String,
        );
        let (_cfg, plan) = analyze_agent(&agent);
        assert_eq!(plan.dups.len(), 0, "last use needs no Dup");
        assert_eq!(plan.drops_after.len(), 0);
        assert_eq!(plan.drops_at_block_exit.len(), 0);
    }

    /// Two consuming uses of the same String. First use needs Dup;
    /// second is the last use and is bare consume.
    #[test]
    fn two_uses_first_gets_dup() {
        // let t = s + s; return t
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
        let (_cfg, plan) = analyze_agent(&agent);
        // Two reads of param 0 in the let; the first is non-last, the
        // second is the last. Plan should Dup once at the let position.
        let dups_at_let = plan.dups.get(&(0, 0)).cloned().unwrap_or_default();
        assert!(
            dups_at_let.contains(&LocalId(0)),
            "param 0 needs a Dup at the let site for its non-last use"
        );
        // Return uses local 1 as last consuming use → no Dup expected.
        assert!(
            plan.dups
                .get(&(0, 1))
                .map_or(true, |s| !s.contains(&LocalId(1))),
            "let-bound t is consumed by return — no Dup"
        );
    }

    /// Unused refcounted parameter → dropped at entry-block exit.
    #[test]
    fn unused_param_drops_at_entry_exit() {
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
        let (_cfg, plan) = analyze_agent(&agent);
        let drops = plan
            .drops_at_block_exit
            .get(&0)
            .cloned()
            .unwrap_or_default();
        assert!(
            drops.contains(&LocalId(0)),
            "unused String param must be scheduled for drop"
        );
    }

    /// `let t = s; ...nothing reads t` → drop t right after the let.
    #[test]
    fn defined_but_never_used_drops_after_let() {
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
        let (_cfg, plan) = analyze_agent(&agent);
        let drops = plan.drops_after.get(&(0, 0)).cloned().unwrap_or_default();
        assert!(
            drops.contains(&LocalId(1)),
            "t defined but never used → drop after the let"
        );
    }

    /// Non-refcounted Int parameter — no plan entries.
    #[test]
    fn int_param_ignored() {
        let agent = make_agent(
            vec![(0, Type::Int)],
            vec![IrStmt::Return {
                value: Some(local_expr(0, Type::Int)),
                span: span(),
            }],
            Type::Int,
        );
        let (_cfg, plan) = analyze_agent(&agent);
        assert_eq!(plan.op_count(), 0, "Int locals never appear in plan");
    }

    /// CFG smoke: an `if` with both branches builds 4 blocks
    /// (entry, then, [else], join) and the join is the post-if cursor.
    #[test]
    fn if_else_builds_four_blocks() {
        let agent = make_agent(
            vec![(0, Type::Bool)],
            vec![
                IrStmt::If {
                    cond: local_expr(0, Type::Bool),
                    then_block: IrBlock {
                        stmts: vec![],
                        span: span(),
                    },
                    else_block: Some(IrBlock {
                        stmts: vec![],
                        span: span(),
                    }),
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
        let (cfg, _plan) = analyze_agent(&agent);
        assert_eq!(cfg.blocks.len(), 4, "entry + then + else + join");
        // Entry has Branch → then & else both reach join.
        assert!(cfg.blocks[0].successors.contains(&1));
        assert!(cfg.blocks[0].successors.contains(&3));
    }

    /// Determinism: running the analysis twice on equal IR yields
    /// equal plans. This is the property the 17f++ trigger-log relies
    /// on for cross-run replay.
    #[test]
    fn determinism_two_runs_match() {
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
        let (_, p1) = analyze_agent(&mk());
        let (_, p2) = analyze_agent(&mk());
        assert_eq!(p1.op_count(), p2.op_count());
        assert_eq!(
            p1.dups.keys().collect::<Vec<_>>(),
            p2.dups.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            p1.drops_after.keys().collect::<Vec<_>>(),
            p2.drops_after.keys().collect::<Vec<_>>()
        );
    }
}
