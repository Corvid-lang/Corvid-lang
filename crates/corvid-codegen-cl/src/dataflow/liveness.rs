use corvid_resolve::LocalId;
use corvid_types::Type;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{BlockId, Cfg, CfgStmt, LocalRead}; // ---------------------------------------------------------------------------
                                               // Liveness — backward dataflow on the CFG
                                               // ---------------------------------------------------------------------------

/// Per-block live-in / live-out sets for refcounted locals. Used to
/// classify last-uses: a use of `L` at point P is the last use iff
/// `L` is not in the live-out set of P.
/// Per-block live-in / live-out sets for refcounted locals, computed
/// by the backward-dataflow pass at function granularity. Public so
/// downstream passes (17b-6 effect-row-directed RC, 17b-7 latency-
/// aware RC) can consume the same liveness facts instead of
/// duplicating the analysis.
///
/// Use `live_in_at_block(block_id)` / `live_out_at_block(block_id)`
/// accessors rather than reaching into the fields directly — they
/// bounds-check the block id and keep the internal vector
/// representation encapsulated.
#[derive(Debug, Clone, Default)]
pub struct Liveness {
    /// For each block, the set of locals live on entry.
    pub(super) live_in: Vec<BTreeSet<LocalId>>,
    /// For each block, the set of locals live on exit.
    pub(super) live_out: Vec<BTreeSet<LocalId>>,
}

impl Liveness {
    /// Locals that are live AT THE START of block `b` (i.e., some
    /// path reaches `b` and then uses the local before redefining).
    /// Returns an empty set for out-of-range block ids.
    pub fn live_in_at_block(&self, b: BlockId) -> &BTreeSet<LocalId> {
        static EMPTY: std::sync::OnceLock<BTreeSet<LocalId>> = std::sync::OnceLock::new();
        self.live_in
            .get(b)
            .unwrap_or_else(|| EMPTY.get_or_init(BTreeSet::new))
    }

    /// Locals that are live AT THE END of block `b` (i.e., some
    /// successor uses the local before redefining). Returns an
    /// empty set for out-of-range block ids.
    pub fn live_out_at_block(&self, b: BlockId) -> &BTreeSet<LocalId> {
        static EMPTY: std::sync::OnceLock<BTreeSet<LocalId>> = std::sync::OnceLock::new();
        self.live_out
            .get(b)
            .unwrap_or_else(|| EMPTY.get_or_init(BTreeSet::new))
    }

    /// Number of CFG blocks this liveness was computed over.
    pub fn block_count(&self) -> usize {
        self.live_in.len()
    }
}

/// Standard iterative backward liveness. Classic Kildall formulation:
/// live_in(B)  = uses(B) ∪ (live_out(B) − defs(B))
/// live_out(B) = ⋃ live_in(S)  for each successor S
pub(super) fn compute_liveness(cfg: &Cfg, params: &BTreeMap<LocalId, Type>) -> Liveness {
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

pub(super) fn stmt_reads(s: &CfgStmt) -> &[LocalRead] {
    match s {
        CfgStmt::Let { reads, .. }
        | CfgStmt::Expr { reads }
        | CfgStmt::Return { reads }
        | CfgStmt::Branch { reads }
        | CfgStmt::LoopHead { reads, .. }
        | CfgStmt::Other { reads } => reads,
    }
}

pub(super) fn stmt_def(s: &CfgStmt) -> Option<LocalId> {
    match s {
        CfgStmt::Let { lhs, .. } => Some(*lhs),
        CfgStmt::LoopHead { var, .. } => Some(*var),
        _ => None,
    }
}
