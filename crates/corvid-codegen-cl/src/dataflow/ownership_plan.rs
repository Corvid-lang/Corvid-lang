use corvid_resolve::LocalId;
use corvid_types::Type;
use std::collections::{BTreeMap, BTreeSet};

use crate::ownership::is_refcounted;

use super::cfg::IfCfgCoords;
use super::liveness::{stmt_def, stmt_reads};
use super::{BlockId, Cfg, CfgStmt, Liveness, ProgramPoint, ReadKind}; // ---------------------------------------------------------------------------
                                                                      // Ownership plan — what .6b will consume
                                                                      // ---------------------------------------------------------------------------

/// One navigation step from an `IrAgent.body` root down to a specific
/// nested statement. Used by the ownership rewriter to translate `ProgramPoint`
/// coordinates (which live in the flattened CFG) back into mutable
/// positions in the original IR tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IrNavStep {
    /// Index into the current block's `stmts` vector. Always the LAST
    /// step in any path (final statement coordinate).
    Stmt(usize),
    /// Descend into the `then_block` of an `IrStmt::If` at the
    /// position given by the immediately preceding `Stmt` step.
    IfThen,
    /// Descend into the `else_block` of an `IrStmt::If`. Only valid
    /// if the `If` has an else branch.
    IfElse,
    /// Descend into the `body` of an `IrStmt::For`.
    ForBody,
}

/// A navigation path: a sequence of steps from the agent body root
/// to one specific statement. Always ends with a `Stmt(idx)`.
pub type IrPath = Vec<IrNavStep>;

/// The output of the ownership analysis. Per agent, lists every
/// `Dup`/`Drop` the IR rewriter should insert.
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
///   - `ir_paths[(block, pos)]` — the IR-tree coordinate that maps
///     this CFG `ProgramPoint` to a mutable position in
///     `agent.body`. Populated by `analyze_agent`. The IR rewriter
///     consumes this to insert `IrStmt::Dup`/`IrStmt::Drop`.
///
/// Determinism: BTreeMap/BTreeSet throughout. Pass output is stable
/// across runs and across compiler versions — the trigger-log and
/// record/replay story rely on it.
#[derive(Debug, Clone, Default)]
pub struct OwnershipPlan {
    pub dups: BTreeMap<ProgramPoint, BTreeSet<LocalId>>,
    pub drops_after: BTreeMap<ProgramPoint, BTreeSet<LocalId>>,
    pub drops_at_block_exit: BTreeMap<BlockId, BTreeSet<LocalId>>,
    pub ir_paths: BTreeMap<ProgramPoint, IrPath>,
    /// Branch-local drop specialization. For each `If`
    /// statement where a refcounted local dies on only SOME branch
    /// paths, schedule per-branch drops so the local is released on
    /// every path that reaches post-If code.
    ///
    /// Key: `IrPath` pointing to the If statement.
    /// Value: which locals die on which branch edge.
    ///
    /// Applied by `dup_drop::apply_plan` after the main insertions:
    ///   - `then_drops`: append `Drop`s to the tail of the If's
    ///     `then_block`.
    ///   - `fallthrough_drops`: append `Drop`s to the tail of the
    ///     If's `else_block` (synthesizing an else_block if None).
    pub branch_drops: BTreeMap<IrPath, BranchDrops>,
}

/// Per-`If` branch-scoped drop bookkeeping. Each set lists
/// refcounted locals that must be released on the corresponding
/// branch edge to match live-out-of-If with live-in-of-post-If.
#[derive(Debug, Clone, Default)]
pub struct BranchDrops {
    /// Drops to apply at the tail of `then_block`.
    pub then_drops: BTreeSet<LocalId>,
    /// Drops to apply at the tail of `else_block` (or in a
    /// synthesized else_block when the source had `else_block: None`).
    pub fallthrough_drops: BTreeSet<LocalId>,
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

/// Per-`If` per-branch drop analysis.
///
/// For each If statement recorded in `if_cfg_coords`, compute the
/// set of refcounted locals that die on the then-edge (but not the
/// else/fallthrough-edge) and vice versa. Drops on each edge are
/// injected by `dup_drop::apply_plan` at the tail of the
/// corresponding IR branch.
///
/// Algorithm:
///   - `dies_on_then_edge = live_out[cond] - live_in[then_cfg]`
///     → schedule drops at tail of then_block
///   - `dies_on_other_edge = live_out[cond] - live_in[merge_or_else]`
///     → schedule drops at tail of else_block (synthesize if None)
///
/// A local dropped at the tail of a branch MUST NOT be dropped
/// anywhere else on that branch — otherwise double-free. In
/// practice, the analysis only schedules a branch-tail drop when
/// the local is NOT used in that branch (live_in of that branch's
/// CFG entry doesn't contain it), which means no in-branch drop
/// exists for it. Safe by construction.
pub(super) fn build_branch_drops(
    cfg: &Cfg,
    liveness: &Liveness,
    if_cfg_coords: &BTreeMap<IrPath, IfCfgCoords>,
    _params: &BTreeMap<LocalId, Type>,
) -> BTreeMap<IrPath, BranchDrops> {
    let mut out: BTreeMap<IrPath, BranchDrops> = BTreeMap::new();
    for (ir_path, coords) in if_cfg_coords {
        let live_out_cond = &liveness.live_out[coords.cond_block];
        let live_in_then = &liveness.live_in[coords.then_cfg];
        let live_in_other = &liveness.live_in[coords.merge_or_else_cfg];

        let mut branch = BranchDrops::default();

        // then-edge: local live going into the branch but NOT used
        // in the then-branch → dies on this edge.
        for &l in live_out_cond {
            if !live_in_then.contains(&l) {
                branch.then_drops.insert(l);
            }
        }
        // other-edge: same idea for the else/fallthrough branch.
        for &l in live_out_cond {
            if !live_in_other.contains(&l) {
                branch.fallthrough_drops.insert(l);
            }
        }

        let _ = coords.has_else; // consumed by apply_plan
        let _ = cfg; // silence unused; kept for future per-block access

        if !branch.then_drops.is_empty() || !branch.fallthrough_drops.is_empty() {
            out.insert(ir_path.clone(), branch);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Plan construction
// ---------------------------------------------------------------------------

pub(super) fn build_plan(
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
                let read_again_in_stmt = reads.iter().skip(read_idx + 1).any(|r2| r2.local_id == l);
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
