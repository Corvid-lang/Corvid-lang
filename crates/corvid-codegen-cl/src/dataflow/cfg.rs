use corvid_ir::{IrAgent, IrBlock, IrExpr, IrExprKind, IrStmt};
use corvid_resolve::LocalId;
use corvid_types::Type;
use std::collections::BTreeMap;

use crate::ownership::is_refcounted;

use super::{IrNavStep, IrPath}; // ---------------------------------------------------------------------------
                                // CFG types
                                // ---------------------------------------------------------------------------

/// Sequential index into `Cfg::blocks`. Stable for the lifetime of a
/// `Cfg`. Using a usize keeps map/set keys cheap; not a newtype because
/// the analysis is internal and the extra type-wrapping overhead would
/// only obscure the dataflow code below.
pub type BlockId = usize;

/// Statement index within a `CfgBlock::stmts`.
pub type StmtPos = usize;

/// A coordinate identifying one statement in the CFG.
pub type ProgramPoint = (BlockId, StmtPos);

/// Linearized control-flow graph derived from an `IrAgent` body.
#[derive(Debug, Clone)]
pub struct Cfg {
    pub blocks: Vec<CfgBlock>,
    pub entry: BlockId,
}

/// A basic block: a straight-line sequence of statements with one
/// entry point and an explicit list of successor blocks at its tail.
#[derive(Debug, Clone)]
pub struct CfgBlock {
    /// Source-order statements in this basic block.
    pub stmts: Vec<CfgStmt>,
    /// Block IDs that control flow may transfer to from the end of
    /// this block. Empty for a block ending in `Return` or for
    /// trailing exit blocks.
    pub successors: Vec<BlockId>,
}

/// A flattened statement. Holds the local reads we extracted from
/// the corresponding `IrStmt`. We don't carry the original
/// expression tree — the analysis only needs reads + defs + control
/// flow at this layer. The ownership rewriter re-walks the IR alongside the
/// plan to actually insert `Dup`/`Drop` ops.
#[derive(Debug, Clone)]
pub enum CfgStmt {
    /// `let lhs = expr`. Defines `lhs`. Reads listed in `reads`.
    Let {
        lhs: LocalId,
        lhs_ty: Type,
        reads: Vec<LocalRead>,
    },
    /// Side-effect or control-flow without a binding.
    Expr { reads: Vec<LocalRead> },
    /// `return expr?` — exits the function. Reads listed.
    Return { reads: Vec<LocalRead> },
    /// Conditional branch (for `if`). Reads the condition's locals.
    /// Successor blocks come from the parent `CfgBlock::successors`.
    Branch { reads: Vec<LocalRead> },
    /// Loop entry — reads the iterable's locals; introduces the loop
    /// variable. Cfg edges out of the parent block describe the body
    /// + after-loop continuation.
    LoopHead {
        var: LocalId,
        var_ty: Type,
        reads: Vec<LocalRead>,
    },
    /// Approve, break, continue, pass — included for completeness;
    /// either pure control or carries a small read set.
    Other { reads: Vec<LocalRead> },
}

/// One local-variable use recorded at a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRead {
    pub local_id: LocalId,
    pub kind: ReadKind,
}

/// How the surrounding expression treats the read value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    /// The use will consume the value (e.g. RHS of `let`, `return`,
    /// passed to a parameter the callee takes Owned). Each non-last
    /// consuming use needs a `Dup` before it; the last consuming use
    /// closes the lifetime.
    Owned,
    /// The use only inspects the value (e.g. accessed for a field
    /// read where the surrounding lowering already retains the
    /// field, callee-side borrow). Doesn't transfer the +1.
    Borrowed,
}

pub(super) struct BuiltCfg {
    pub cfg: Cfg,
    pub ir_paths: BTreeMap<ProgramPoint, IrPath>,
    pub if_cfg_coords: BTreeMap<IrPath, IfCfgCoords>,
}

pub(super) fn build_cfg(agent: &IrAgent) -> BuiltCfg {
    let mut builder = CfgBuilder::new();
    let entry = builder.alloc_block();
    builder.lower_block(&agent.body, entry, Vec::new());
    BuiltCfg {
        cfg: Cfg {
            blocks: builder.blocks,
            entry,
        },
        ir_paths: builder.ir_paths,
        if_cfg_coords: builder.if_cfg_coords,
    }
}
// ---------------------------------------------------------------------------
// CFG construction
// ---------------------------------------------------------------------------

struct CfgBuilder {
    blocks: Vec<CfgBlock>,
    /// Maps each CFG `ProgramPoint` to the IR-tree path that produced
    /// it. Populated as `lower_block` walks; consumed by
    /// `analyze_agent` and merged into the returned `OwnershipPlan`.
    ir_paths: BTreeMap<ProgramPoint, IrPath>,
    /// For each lowered `IrStmt::If`, record the CFG
    /// coordinates needed to compute per-branch drops later:
    ///   - `cond_block`: the CFG block ending in the Branch (same as
    ///     the block where the If's IrPath lives).
    ///   - `then_cfg`: first CFG block of the then-branch.
    ///   - `merge_or_else_cfg`: the CFG block control flows to when
    ///     the cond is false (either the synthesized merge block for
    ///     a no-else If, or the first block of the else-branch).
    ///   - `has_else`: whether the source If had an else-block.
    /// Keyed by `IrPath` to the If statement.
    if_cfg_coords: BTreeMap<IrPath, IfCfgCoords>,
}

#[derive(Debug, Clone)]
pub(super) struct IfCfgCoords {
    pub(super) cond_block: BlockId,
    pub(super) then_cfg: BlockId,
    pub(super) merge_or_else_cfg: BlockId,
    pub(super) has_else: bool,
}

impl CfgBuilder {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            ir_paths: BTreeMap::new(),
            if_cfg_coords: BTreeMap::new(),
        }
    }

    fn alloc_block(&mut self) -> BlockId {
        let id = self.blocks.len();
        self.blocks.push(CfgBlock {
            stmts: Vec::new(),
            successors: Vec::new(),
        });
        id
    }

    fn push_stmt(&mut self, b: BlockId, s: CfgStmt, ir_path: IrPath) {
        let pos = self.blocks[b].stmts.len();
        self.blocks[b].stmts.push(s);
        self.ir_paths.insert((b, pos), ir_path);
    }

    fn add_succ(&mut self, from: BlockId, to: BlockId) {
        if !self.blocks[from].successors.contains(&to) {
            self.blocks[from].successors.push(to);
        }
    }

    /// Lower an `IrBlock` into the CFG starting at `current`. Returns
    /// the block id where control flow rests after the block (for
    /// chaining with subsequent siblings). Returns `None` if control
    /// definitely exits (the block ends in `Return`).
    ///
    /// `parent_path` is the navigation path from `agent.body` down to
    /// (but not including) the statements being lowered now. Each
    /// statement's full path is `parent_path ++ [Stmt(idx)]` for the
    /// statement at index `idx` within `block.stmts`.
    fn lower_block(
        &mut self,
        block: &IrBlock,
        current: BlockId,
        parent_path: IrPath,
    ) -> Option<BlockId> {
        let mut cur = current;
        for (ir_idx, stmt) in block.stmts.iter().enumerate() {
            let mut my_path = parent_path.clone();
            my_path.push(IrNavStep::Stmt(ir_idx));
            match stmt {
                IrStmt::Let {
                    local_id,
                    value,
                    ty,
                    ..
                } => {
                    let reads = collect_reads(value, true);
                    self.push_stmt(
                        cur,
                        CfgStmt::Let {
                            lhs: *local_id,
                            lhs_ty: ty.clone(),
                            reads,
                        },
                        my_path,
                    );
                }
                IrStmt::Expr { expr, .. } => {
                    let reads = collect_reads(expr, true);
                    self.push_stmt(cur, CfgStmt::Expr { reads }, my_path);
                }
                IrStmt::Yield { value, .. } => {
                    let reads = collect_reads(value, true);
                    self.push_stmt(cur, CfgStmt::Expr { reads }, my_path);
                }
                IrStmt::Return { value, .. } => {
                    let reads = match value {
                        Some(e) => collect_reads(e, true),
                        None => Vec::new(),
                    };
                    self.push_stmt(cur, CfgStmt::Return { reads }, my_path);
                    // No successor — return exits.
                    return None;
                }
                IrStmt::If {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => {
                    let if_ir_path = my_path.clone();
                    let cond_block = cur;
                    let cond_reads = collect_reads(cond, false);
                    self.push_stmt(cur, CfgStmt::Branch { reads: cond_reads }, my_path.clone());
                    let then_id = self.alloc_block();
                    self.add_succ(cur, then_id);
                    let mut then_parent = my_path.clone();
                    then_parent.push(IrNavStep::IfThen);
                    let after_then = self.lower_block(then_block, then_id, then_parent);

                    let join = self.alloc_block();
                    if let Some(at) = after_then {
                        self.add_succ(at, join);
                    }
                    let (merge_or_else_cfg, has_else) = if let Some(eb) = else_block {
                        let else_id = self.alloc_block();
                        self.add_succ(cur, else_id);
                        let mut else_parent = my_path.clone();
                        else_parent.push(IrNavStep::IfElse);
                        let after_else = self.lower_block(eb, else_id, else_parent);
                        if let Some(ae) = after_else {
                            self.add_succ(ae, join);
                        }
                        (else_id, true)
                    } else {
                        // No else branch: control may fall through.
                        self.add_succ(cur, join);
                        (join, false)
                    };
                    // Record CFG coords for 17b-2 per-branch drop analysis.
                    self.if_cfg_coords.insert(
                        if_ir_path,
                        IfCfgCoords {
                            cond_block,
                            then_cfg: then_id,
                            merge_or_else_cfg,
                            has_else,
                        },
                    );
                    cur = join;
                }
                IrStmt::For {
                    var_local,
                    var_name: _,
                    iter,
                    body,
                    ..
                } => {
                    // For-loop iter is BORROWED when it's a bare Local
                    // (the codegen peephole `lower_container_maybe_borrowed`
                    // reads the Variable without retaining; the iter's
                    // +1 stays with the Local, not the loop). Classify
                    // as Borrowed so the pass schedules a Drop after
                    // last-use (i.e., after the loop) rather than
                    // assuming consumption.
                    //
                    // For a non-bare iter (list literal, call result),
                    // codegen's for-loop epilogue releases the temp
                    // unconditionally — the pass doesn't need to track
                    // it. Classifying as Borrowed here still yields
                    // correct behavior because the classification only
                    // affects Local reads.
                    let iter_reads = collect_reads(iter, false);
                    self.push_stmt(
                        cur,
                        CfgStmt::LoopHead {
                            var: *var_local,
                            var_ty: iter_element_type(iter),
                            reads: iter_reads,
                        },
                        my_path.clone(),
                    );
                    let body_id = self.alloc_block();
                    let after_loop = self.alloc_block();
                    // Loop edges: head → body, body → head (back-edge),
                    // head → after_loop (exit on iterator empty).
                    self.add_succ(cur, body_id);
                    self.add_succ(cur, after_loop);
                    let mut body_parent = my_path.clone();
                    body_parent.push(IrNavStep::ForBody);
                    let after_body = self.lower_block(body, body_id, body_parent);
                    if let Some(ab) = after_body {
                        self.add_succ(ab, cur);
                    }
                    cur = after_loop;
                }
                IrStmt::Approve { args, .. } => {
                    let mut reads = Vec::new();
                    for a in args {
                        reads.extend(collect_reads(a, false));
                    }
                    self.push_stmt(cur, CfgStmt::Other { reads }, my_path);
                }
                IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => {
                    self.push_stmt(cur, CfgStmt::Other { reads: Vec::new() }, my_path);
                }
                // Dup/Drop should not appear in input IR for .6a — the
                // pass that inserts them is .6b, which hasn't run yet.
                // Treat as a no-op for forward compatibility (so .6b
                // can run the analysis on its own output for
                // verification without crashing).
                IrStmt::Dup { .. } | IrStmt::Drop { .. } => {
                    self.push_stmt(cur, CfgStmt::Other { reads: Vec::new() }, my_path);
                }
            }
        }
        Some(cur)
    }
}

/// Best-effort element type for an iterator expression. List<T> → T;
/// other shapes return Nothing as a placeholder. The element-type only
/// matters for deciding whether the loop var is refcounted — we err on
/// the side of "not refcounted" if we can't tell, which matches what
/// the existing for-loop lowering does.
fn iter_element_type(e: &IrExpr) -> Type {
    match &e.ty {
        Type::List(inner) => (**inner).clone(),
        _ => Type::Nothing,
    }
}

// ---------------------------------------------------------------------------
// Read collection
// ---------------------------------------------------------------------------

/// Walk `expr` and produce the list of refcounted-local reads it
/// performs, in source order. Each read is classified as Owned
/// (consumed) or Borrowed (only inspected) based on the immediately
/// surrounding context.
///
/// `consumed` is the classification for a *bare* `Local` read at THIS
/// expression node. Subexpressions inherit a context-specific value.
///
/// The classification heuristic for .6a:
///   - RHS of `let`, value of `return`, top-level `Expr` statement,
///     LoopHead's iter — `consumed = true`.
///   - Argument of an `If` cond, an `Approve` arg, or a Borrowed
///     callee parameter — `consumed = false`.
///   - For composite expressions (FieldAccess, Index, BinOp, UnOp,
///     Call, List), inner Local reads are *consumed by the composite*
///     unless the composite is itself in a Borrowed context. We model
///     the conservative case: composite operands are Owned when their
///     outer context consumes the result.
fn collect_reads(expr: &IrExpr, consumed: bool) -> Vec<LocalRead> {
    let mut out = Vec::new();
    walk_expr(expr, consumed, &mut out);
    out
}

fn walk_expr(expr: &IrExpr, consumed: bool, out: &mut Vec<LocalRead>) {
    match &expr.kind {
        IrExprKind::Local { local_id, .. } => {
            if is_refcounted(&expr.ty) {
                out.push(LocalRead {
                    local_id: *local_id,
                    kind: if consumed {
                        ReadKind::Owned
                    } else {
                        ReadKind::Borrowed
                    },
                });
            }
        }
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } | IrExprKind::OptionNone => {}
        IrExprKind::FieldAccess { target, .. } => {
            // Target is borrowed by the field access — it's inspected,
            // a read of one field, the surrounding context owns the
            // resulting field value, not the target itself.
            walk_expr(target, false, out);
        }
        IrExprKind::UnwrapGrounded { value } => walk_expr(value, true, out),
        IrExprKind::Index { target, index } => {
            walk_expr(target, false, out);
            walk_expr(index, true, out);
        }
        IrExprKind::BinOp { left, right, .. } | IrExprKind::WrappingBinOp { left, right, .. } => {
            // String concat consumes both operands (releases at
            // codegen line 3336–3340). Other BinOps on primitives
            // have no refcounted operands to track.
            walk_expr(left, true, out);
            walk_expr(right, true, out);
        }
        IrExprKind::UnOp { operand, .. } | IrExprKind::WrappingUnOp { operand, .. } => {
            walk_expr(operand, true, out);
        }
        IrExprKind::Call { kind, args, .. } => {
            // Call-arg ownership semantics depend on the call kind
            // (Scoped-C fix per 17b-1b.6d-2b):
            //
            //   - Tool / Prompt: the FFI bridge on the other side
            //     borrows refcounted args for the duration of the
            //     call and does NOT take a +1. Caller retains
            //     ownership; analysis must treat args as Borrowed so
            //     the pass schedules a Drop after last-use (not
            //     before, which would assume consumption).
            //
            //   - Agent: depends on the callee's borrow_sig. Without
            //     a sig in hand here (analyzer doesn't thread it
            //     through), conservatively treat as Owned — matches
            //     pre-17b behavior. The codegen peephole at the
            //     Agent call site consults borrow_sig directly.
            //
            //   - StructConstructor: args are consumed (fields own
            //     the values). Owned.
            //
            //   - Unknown: treat conservatively — Borrowed avoids
            //     scheduling a consumption-style Drop on an arg we
            //     can't prove is actually consumed.
            let args_consumed = match kind {
                corvid_ir::IrCallKind::Tool { .. }
                | corvid_ir::IrCallKind::Prompt { .. }
                | corvid_ir::IrCallKind::Fixture { .. }
                | corvid_ir::IrCallKind::Unknown => false,
                corvid_ir::IrCallKind::Agent { .. }
                | corvid_ir::IrCallKind::StructConstructor { .. } => true,
            };
            for a in args {
                walk_expr(a, args_consumed, out);
            }
        }
        IrExprKind::List { items } => {
            for item in items {
                walk_expr(item, true, out);
            }
        }
        IrExprKind::WeakNew { strong } => walk_expr(strong, true, out),
        IrExprKind::WeakUpgrade { weak } => walk_expr(weak, false, out),
        IrExprKind::StreamSplitBy { stream, .. } => walk_expr(stream, false, out),
        IrExprKind::StreamMerge { groups, .. } => walk_expr(groups, false, out),
        IrExprKind::StreamOrderedBy { stream, .. } => walk_expr(stream, false, out),
        IrExprKind::StreamResumeToken { stream } => walk_expr(stream, false, out),
        IrExprKind::ResumeStream { token, .. } => walk_expr(token, false, out),
        IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::Ask { prompt: inner, .. }
        | IrExprKind::Choose { options: inner }
        | IrExprKind::TryPropagate { inner } => walk_expr(inner, true, out),
        IrExprKind::TryRetry { body, .. } => walk_expr(body, consumed, out),
        IrExprKind::Replay {
            trace,
            arms,
            else_body,
        } => {
            // Liveness across replay arms: the trace expression is
            // consumed by the runtime to locate the trace file;
            // exactly one arm body OR the else body executes (first
            // match wins per E-runtime semantics), so conservatively
            // treat every arm body + the else body as potential
            // continuations that inherit `consumed` from the enclosing
            // context. `TraceId` is not refcounted, so the trace
            // expression itself doesn't contribute local reads
            // through its own type, but nested sub-expressions might.
            walk_expr(trace, true, out);
            for arm in arms {
                walk_expr(&arm.body, consumed, out);
            }
            walk_expr(else_body, consumed, out);
        }
    }
}
