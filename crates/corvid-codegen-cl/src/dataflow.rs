//! Slice 17b-1b.6a — CFG + liveness + ownership dataflow analysis.
//!
//! READ-ONLY in this commit. The analysis runs over an `IrAgent`,
//! computes a per-local ownership plan (where to insert `Dup`, where
//! to insert `Drop`), and returns it as a structured value. It does
//! NOT mutate the IR or change codegen. Slice 17b-1b.6b will consume
//! the plan to actually insert `IrStmt::Dup` / `IrStmt::Drop`.
//! Slice 17b-1b.6c will then delete the scattered
//! `emit_retain` / `emit_release` sites in `lowering.rs` plus the
//! four 17b-1b.2..5 peepholes.
//!
//! Design choices pinned in pre-phase chat:
//!   - Path B: ship as three sub-slices (.6a / .6b / .6c) so each is
//!     reviewable; the monolithic claim is honored in the final state.
//!   - Dataflow precision (not linear scan; not full Perceus).
//!   - The 17f++ verifier (`CORVID_GC_VERIFY=abort`) audits the result
//!     against the runtime contract in .6c.
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
//! ## Non-goals for .6a
//!
//! - Does not insert `Dup`/`Drop` into the IR (.6b's job).
//! - Does not change codegen (.6c's job).
//! - Does not handle `Weak<T>` ownership (slice 17g; deliberately
//!   shipped in parallel by Developer B and joined later).
//! - Does not perform any of 17b-3..17b-7 advanced optimizations.

use corvid_ir::{IrAgent, IrBlock, IrExpr, IrExprKind, IrStmt};
use corvid_resolve::LocalId;
use corvid_types::Type;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::ownership::is_refcounted;

// ---------------------------------------------------------------------------
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
/// flow at this layer. Slice .6b will re-walk the IR alongside the
/// plan to actually insert `Dup`/`Drop` ops.
#[derive(Debug, Clone)]
pub enum CfgStmt {
    /// `let lhs = expr`. Defines `lhs`. Reads listed in `reads`.
    Let { lhs: LocalId, lhs_ty: Type, reads: Vec<LocalRead> },
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
    LoopHead { var: LocalId, var_ty: Type, reads: Vec<LocalRead> },
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

// ---------------------------------------------------------------------------
// Ownership plan — what .6b will consume
// ---------------------------------------------------------------------------

/// The output of this slice. Per agent, lists every Dup/Drop the
/// future .6b pass should insert into the IR.
///
/// Shape:
///   - `dups[(block, pos)]` — set of locals to Dup immediately BEFORE
///     statement at that position.
///   - `drops_after[(block, pos)]` — set of locals to Drop immediately
///     AFTER statement at that position.
///   - `drops_at_block_exit[block]` — set of locals to Drop at the
///     trailing edge of `block` (used for scope-exit cleanup of locals
///     that never reach a use, or for the cleanup at the end of `if`
///     branches when the local has different last-uses on different
///     paths).
///
/// Determinism: BTreeMap/BTreeSet throughout. Pass output is stable
/// across runs and across compiler versions — the 17f++ trigger-log
/// + record/replay story relies on it.
#[derive(Debug, Clone, Default)]
pub struct OwnershipPlan {
    pub dups: BTreeMap<ProgramPoint, BTreeSet<LocalId>>,
    pub drops_after: BTreeMap<ProgramPoint, BTreeSet<LocalId>>,
    pub drops_at_block_exit: BTreeMap<BlockId, BTreeSet<LocalId>>,
}

impl OwnershipPlan {
    pub fn dup_at(&mut self, p: ProgramPoint, l: LocalId) {
        self.dups.entry(p).or_default().insert(l);
    }
    pub fn drop_after(&mut self, p: ProgramPoint, l: LocalId) {
        self.drops_after.entry(p).or_default().insert(l);
    }
    pub fn drop_block_exit(&mut self, b: BlockId, l: LocalId) {
        self.drops_at_block_exit.entry(b).or_default().insert(l);
    }
    /// Total RC ops the plan would emit. Useful for tests + benchmarks.
    pub fn op_count(&self) -> usize {
        let dups: usize = self.dups.values().map(|s| s.len()).sum();
        let drops_a: usize = self.drops_after.values().map(|s| s.len()).sum();
        let drops_e: usize = self.drops_at_block_exit.values().map(|s| s.len()).sum();
        dups + drops_a + drops_e
    }
}

// ---------------------------------------------------------------------------
// Public entry
// ---------------------------------------------------------------------------

/// Run the .6a analysis on a single agent. Returns the CFG + the
/// ownership plan. Pure: does not mutate `agent`.
pub fn analyze_agent(agent: &IrAgent) -> (Cfg, OwnershipPlan) {
    let mut builder = CfgBuilder::new();
    let entry = builder.alloc_block();
    builder.lower_block(&agent.body, entry);
    let cfg = Cfg { blocks: builder.blocks, entry };

    // Parameters are defined at function entry. Refcounted parameters
    // start owned (for now — borrow-sig-driven parameter elision is
    // already handled at codegen by the existing borrow_sig consumer
    // and survives this slice unchanged).
    let mut param_locals: BTreeMap<LocalId, Type> = BTreeMap::new();
    for p in &agent.params {
        if is_refcounted(&p.ty) {
            param_locals.insert(p.local_id, p.ty.clone());
        }
    }

    let liveness = compute_liveness(&cfg, &param_locals);
    let plan = build_plan(&cfg, &liveness, &param_locals);
    (cfg, plan)
}

// ---------------------------------------------------------------------------
// CFG construction
// ---------------------------------------------------------------------------

struct CfgBuilder {
    blocks: Vec<CfgBlock>,
}

impl CfgBuilder {
    fn new() -> Self { Self { blocks: Vec::new() } }

    fn alloc_block(&mut self) -> BlockId {
        let id = self.blocks.len();
        self.blocks.push(CfgBlock { stmts: Vec::new(), successors: Vec::new() });
        id
    }

    fn push_stmt(&mut self, b: BlockId, s: CfgStmt) {
        self.blocks[b].stmts.push(s);
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
    fn lower_block(&mut self, block: &IrBlock, current: BlockId) -> Option<BlockId> {
        let mut cur = current;
        for stmt in &block.stmts {
            match stmt {
                IrStmt::Let { local_id, value, ty, .. } => {
                    let reads = collect_reads(value, true);
                    self.push_stmt(cur, CfgStmt::Let {
                        lhs: *local_id,
                        lhs_ty: ty.clone(),
                        reads,
                    });
                }
                IrStmt::Expr { expr, .. } => {
                    let reads = collect_reads(expr, true);
                    self.push_stmt(cur, CfgStmt::Expr { reads });
                }
                IrStmt::Return { value, .. } => {
                    let reads = match value {
                        Some(e) => collect_reads(e, true),
                        None => Vec::new(),
                    };
                    self.push_stmt(cur, CfgStmt::Return { reads });
                    // No successor — return exits.
                    return None;
                }
                IrStmt::If { cond, then_block, else_block, .. } => {
                    let cond_reads = collect_reads(cond, false);
                    self.push_stmt(cur, CfgStmt::Branch { reads: cond_reads });
                    let then_id = self.alloc_block();
                    self.add_succ(cur, then_id);
                    let after_then = self.lower_block(then_block, then_id);

                    let join = self.alloc_block();
                    if let Some(at) = after_then {
                        self.add_succ(at, join);
                    }
                    if let Some(eb) = else_block {
                        let else_id = self.alloc_block();
                        self.add_succ(cur, else_id);
                        let after_else = self.lower_block(eb, else_id);
                        if let Some(ae) = after_else {
                            self.add_succ(ae, join);
                        }
                    } else {
                        // No else branch: control may fall through.
                        self.add_succ(cur, join);
                    }
                    cur = join;
                }
                IrStmt::For { var_local, var_name: _, iter, body, .. } => {
                    let iter_reads = collect_reads(iter, true);
                    self.push_stmt(cur, CfgStmt::LoopHead {
                        var: *var_local,
                        var_ty: iter_element_type(iter),
                        reads: iter_reads,
                    });
                    let body_id = self.alloc_block();
                    let after_loop = self.alloc_block();
                    // Loop edges: head → body, body → head (back-edge),
                    // head → after_loop (exit on iterator empty).
                    self.add_succ(cur, body_id);
                    self.add_succ(cur, after_loop);
                    let after_body = self.lower_block(body, body_id);
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
                    self.push_stmt(cur, CfgStmt::Other { reads });
                }
                IrStmt::Break { .. }
                | IrStmt::Continue { .. }
                | IrStmt::Pass { .. } => {
                    self.push_stmt(cur, CfgStmt::Other { reads: Vec::new() });
                }
                // Dup/Drop should not appear in input IR for .6a — the
                // pass that inserts them is .6b, which hasn't run yet.
                // Treat as a no-op for forward compatibility (so .6b
                // can run the analysis on its own output for
                // verification without crashing).
                IrStmt::Dup { .. } | IrStmt::Drop { .. } => {
                    self.push_stmt(cur, CfgStmt::Other { reads: Vec::new() });
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
                    kind: if consumed { ReadKind::Owned } else { ReadKind::Borrowed },
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
        IrExprKind::Index { target, index } => {
            walk_expr(target, false, out);
            walk_expr(index, true, out);
        }
        IrExprKind::BinOp { left, right, .. } => {
            // String concat consumes both operands (releases at
            // codegen line 3336–3340). Other BinOps on primitives
            // have no refcounted operands to track.
            walk_expr(left, true, out);
            walk_expr(right, true, out);
        }
        IrExprKind::UnOp { operand, .. } => {
            walk_expr(operand, true, out);
        }
        IrExprKind::Call { args, .. } => {
            // Without per-callee borrow-sig propagation here (handled
            // by the existing partial 17b-1b.1 pass and the codegen
            // peepholes), we conservatively classify args as Owned.
            // .6b will refine this when it consumes the per-callee
            // borrow_sig from `AgentSummary`.
            for a in args {
                walk_expr(a, true, out);
            }
        }
        IrExprKind::List { items } => {
            for item in items {
                walk_expr(item, true, out);
            }
        }
        IrExprKind::WeakNew { strong } => walk_expr(strong, true, out),
        IrExprKind::WeakUpgrade { weak } => walk_expr(weak, false, out),
        IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => walk_expr(inner, true, out),
        IrExprKind::TryRetry { body, .. } => walk_expr(body, consumed, out),
    }
}

// ---------------------------------------------------------------------------
// Liveness — backward dataflow on the CFG
// ---------------------------------------------------------------------------

/// Per-block live-in / live-out sets for refcounted locals. Used to
/// classify last-uses: a use of `L` at point P is the last use iff
/// `L` is not in the live-out set of P.
#[derive(Debug, Clone, Default)]
struct Liveness {
    /// For each block, the set of locals live on entry.
    live_in: Vec<BTreeSet<LocalId>>,
    /// For each block, the set of locals live on exit.
    live_out: Vec<BTreeSet<LocalId>>,
}

/// Standard iterative backward liveness. Classic Kildall formulation:
/// live_in(B)  = uses(B) ∪ (live_out(B) − defs(B))
/// live_out(B) = ⋃ live_in(S)  for each successor S
fn compute_liveness(cfg: &Cfg, params: &BTreeMap<LocalId, Type>) -> Liveness {
    let n = cfg.blocks.len();
    let mut live_in = vec![BTreeSet::<LocalId>::new(); n];
    let mut live_out = vec![BTreeSet::<LocalId>::new(); n];

    let (uses, defs) = block_use_def(cfg);

    let mut worklist: VecDeque<BlockId> = (0..n).collect();
    while let Some(b) = worklist.pop_front() {
        let mut new_out = BTreeSet::new();
        for &s in &cfg.blocks[b].successors {
            for &l in &live_in[s] {
                new_out.insert(l);
            }
        }
        let mut new_in = uses[b].clone();
        for &l in &new_out {
            if !defs[b].contains(&l) {
                new_in.insert(l);
            }
        }
        if new_in != live_in[b] || new_out != live_out[b] {
            live_in[b] = new_in;
            live_out[b] = new_out;
            // Re-process predecessors.
            for (pb, pblk) in cfg.blocks.iter().enumerate() {
                if pblk.successors.contains(&b) {
                    worklist.push_back(pb);
                }
            }
        }
    }

    // Parameters appear "live in" at the entry block, but they might
    // not be USED by the body. We don't seed them into live_in here —
    // unused params get dropped at scope exit by the plan-builder
    // below, which is the correct behavior.
    let _ = params;
    Liveness { live_in, live_out }
}

/// Per-block: locals that have a use BEFORE any def in the block,
/// and locals that have at least one def in the block.
fn block_use_def(cfg: &Cfg) -> (Vec<BTreeSet<LocalId>>, Vec<BTreeSet<LocalId>>) {
    let n = cfg.blocks.len();
    let mut uses = vec![BTreeSet::<LocalId>::new(); n];
    let mut defs = vec![BTreeSet::<LocalId>::new(); n];
    for (b, blk) in cfg.blocks.iter().enumerate() {
        for stmt in &blk.stmts {
            for r in stmt_reads(stmt) {
                if !defs[b].contains(&r.local_id) {
                    uses[b].insert(r.local_id);
                }
            }
            if let Some(d) = stmt_def(stmt) {
                defs[b].insert(d);
            }
        }
    }
    (uses, defs)
}

fn stmt_reads(s: &CfgStmt) -> &[LocalRead] {
    match s {
        CfgStmt::Let { reads, .. }
        | CfgStmt::Expr { reads }
        | CfgStmt::Return { reads }
        | CfgStmt::Branch { reads }
        | CfgStmt::LoopHead { reads, .. }
        | CfgStmt::Other { reads } => reads,
    }
}

fn stmt_def(s: &CfgStmt) -> Option<LocalId> {
    match s {
        CfgStmt::Let { lhs, .. } => Some(*lhs),
        CfgStmt::LoopHead { var, .. } => Some(*var),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Plan construction
// ---------------------------------------------------------------------------

fn build_plan(
    cfg: &Cfg,
    liveness: &Liveness,
    params: &BTreeMap<LocalId, Type>,
) -> OwnershipPlan {
    let mut plan = OwnershipPlan::default();

    // Walk every statement; classify each consuming use as last/non-last.
    for (b, blk) in cfg.blocks.iter().enumerate() {
        // Track live-after the current statement, starting from
        // live_out of the block and walking backward.
        let mut live_after_next = liveness.live_out[b].clone();

        for pos in (0..blk.stmts.len()).rev() {
            let stmt = &blk.stmts[pos];
            let live_after_this = live_after_next.clone();

            // Defs kill liveness for the local before this statement.
            let mut live_before = live_after_this.clone();
            if let Some(d) = stmt_def(stmt) {
                live_before.remove(&d);
            }

            // Classify each read. For statements with multiple reads
            // of the same local (e.g. `s + s`), only the LAST read in
            // source order can be the local's last use; earlier reads
            // need a Dup even if the local is not live after the
            // statement, because the later read in the same statement
            // still consumes a refcount.
            let reads = stmt_reads(stmt);
            for (read_idx, r) in reads.iter().enumerate() {
                let l = r.local_id;
                // Is `l` read again LATER in this same statement?
                let read_again_in_stmt = reads
                    .iter()
                    .skip(read_idx + 1)
                    .any(|r2| r2.local_id == l);
                // A use is "last" iff (no later in-statement read of
                // it) AND (not live after the statement on any
                // successor path).
                let is_last = !read_again_in_stmt && !live_after_this.contains(&l);

                live_before.insert(l);

                match r.kind {
                    ReadKind::Owned => {
                        if !is_last {
                            // Non-last consuming use → Dup before this
                            // statement to preserve the local for later
                            // uses while still handing a +1 to this consumer.
                            plan.dup_at((b, pos), l);
                        }
                        // Last consuming use needs no Dup; this use
                        // takes the final +1 and closes the local.
                    }
                    ReadKind::Borrowed => {
                        if is_last {
                            // Last borrowed use → drop after, since
                            // borrowed reads don't transfer ownership.
                            plan.drop_after((b, pos), l);
                        }
                    }
                }
            }

            live_after_next = live_before;
        }

        // After the backward scan, `live_after_next` holds the live-in
        // set for this block. Any local DEFINED in this block but never
        // used (i.e. defined but not in any read above) is currently
        // not represented anywhere — handle that case below.
    }

    // Locals defined-but-never-used → drop right after the defining Let.
    // Walk forward, find each Let whose `lhs` is refcounted and never
    // shows up in any read in the function. Drop after that Let.
    let mut all_reads: BTreeSet<LocalId> = BTreeSet::new();
    for blk in &cfg.blocks {
        for stmt in &blk.stmts {
            for r in stmt_reads(stmt) {
                all_reads.insert(r.local_id);
            }
        }
    }
    for (b, blk) in cfg.blocks.iter().enumerate() {
        for (pos, stmt) in blk.stmts.iter().enumerate() {
            if let CfgStmt::Let { lhs, lhs_ty, .. } = stmt {
                if is_refcounted(lhs_ty) && !all_reads.contains(lhs) {
                    plan.drop_after((b, pos), *lhs);
                }
            }
            if let CfgStmt::LoopHead { var, var_ty, .. } = stmt {
                if is_refcounted(var_ty) && !all_reads.contains(var) {
                    plan.drop_after((b, pos), *var);
                }
            }
        }
    }

    // Refcounted parameters that are never used — drop at entry-block exit.
    let entry = cfg.entry;
    let entry_live_in = &liveness.live_in[entry];
    for (p, _ty) in params {
        if !entry_live_in.contains(p) {
            plan.drop_block_exit(entry, *p);
        }
    }

    plan
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
            plan.dups.get(&(0, 1)).map_or(true, |s| !s.contains(&LocalId(1))),
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
        let drops = plan.drops_at_block_exit.get(&0).cloned().unwrap_or_default();
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
                    then_block: IrBlock { stmts: vec![], span: span() },
                    else_block: Some(IrBlock { stmts: vec![], span: span() }),
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
        assert_eq!(p1.dups.keys().collect::<Vec<_>>(), p2.dups.keys().collect::<Vec<_>>());
        assert_eq!(
            p1.drops_after.keys().collect::<Vec<_>>(),
            p2.drops_after.keys().collect::<Vec<_>>()
        );
    }
}
