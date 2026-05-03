//! Stack receipt, component, delta, and history-facing schema.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{Anomaly, Attribution, DeltaRecord, Verdict};

/// Top-level stack receipt. Carries the dual normal-form / history
/// views, typed anomalies, and component references for Merkle-DAG
/// traversal + verification in the later signing slice.
#[derive(Debug, Clone, Serialize)]
pub(in crate::trace_diff) struct StackReceipt {
    pub schema_version: u32,
    /// sha256 of the sorted per-commit receipt hashes concatenated
    /// with the range spec. Content-addressed: same inputs →
    /// same hash.
    pub stack_hash: String,
    pub base_sha: String,
    pub head_sha: String,
    pub source_path: String,
    pub range_spec: String,
    pub components: Vec<StackComponent>,
    /// Aggregate policy verdict over stack history. Normal form may
    /// cancel a regression later in the stack; the verdict remains
    /// history-sensitive so transient safety regressions surface.
    pub verdict: Verdict,
    /// Net base→head delta set after composition. Canonically
    /// ordered by delta key for byte-stable serialization.
    pub normal_form: Vec<StackDelta>,
    /// Every delta preserved in commit order. Supports
    /// "was X ever true at some point in this stack?" questions.
    pub history: Vec<StackDelta>,
    pub anomalies: Vec<Anomaly>,
    /// Per-trace attribution records. Populated when the driver
    /// runs with `--stack --traces <dir>` (lands in the next
    /// commit of step 3/N); empty for stack receipts composed
    /// without counterfactual replay. Serialized only when
    /// non-empty so the JSON shape of algebra-only stack receipts
    /// stays byte-identical to step 1/N output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributions: Vec<Attribution>,
}

/// Reference to a per-commit receipt that contributed to this
/// stack. Carries hashes rather than the full receipt so consumers
/// traverse the content-addressed cache to materialize.
#[derive(Debug, Clone, Serialize)]
pub(in crate::trace_diff) struct StackComponent {
    pub commit_sha: String,
    pub receipt_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_hash: Option<String>,
    pub signature_status: SignatureStatus,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(in crate::trace_diff) enum SignatureStatus {
    /// Per-commit envelope exists in cache. Verification status is
    /// the caller's responsibility at compose time; this variant
    /// just records that a signature is present.
    Signed,
    /// No envelope for this component. Surfaces as an anomaly
    /// when the later signing slice's `--require-signed-components`
    /// flag is set.
    Unsigned,
    /// Envelope exists but verification didn't complete at compose
    /// time (e.g. key unavailable). Distinct from `Signed` so
    /// callers can distinguish "we know the signature is good"
    /// from "a signature exists but we didn't check it."
    Unknown,
}

/// A delta surviving to the normal form, or preserved in history,
/// with commit-level provenance attached.
#[derive(Debug, Clone, Serialize)]
pub(in crate::trace_diff) struct StackDelta {
    pub key: String,
    pub summary: String,
    /// Commit SHA that pinned this delta's value. For composed
    /// transitions (`A→B ∘ B→C = A→C`), this is the commit of the
    /// final transition — the one whose landing determined the
    /// head value. For lifecycle deltas, the commit of the final
    /// state-determining operation.
    pub introduced_at: String,
}

/// Input to the composer: one commit's contribution to the stack.
/// The caller — typically the trace-diff driver after walking the
/// commit range — materializes this from per-commit receipts that
/// already exist in the cache.
#[derive(Debug, Clone)]
pub(in crate::trace_diff) struct StackInput {
    pub commit_sha: String,
    pub receipt_hash: String,
    pub envelope_hash: Option<String>,
    pub signature_status: SignatureStatus,
    pub deltas: Vec<DeltaRecord>,
}

/// Compute the content-addressed stack hash: sha256 of sorted
/// per-commit receipt hashes concatenated with the range spec.
/// Same inputs → same hash (natural memoization); different
/// inputs → different hash (natural invalidation). Stable across
/// re-composition because receipt hashes are themselves content-
/// addressed — swapping a component for a different-hash
/// component necessarily changes the stack hash.
pub(super) fn compute_stack_hash(components: &[StackComponent], range_spec: &str) -> String {
    let mut receipt_hashes: Vec<&str> =
        components.iter().map(|c| c.receipt_hash.as_str()).collect();
    receipt_hashes.sort_unstable();

    let mut hasher = Sha256::new();
    for h in &receipt_hashes {
        hasher.update(h.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"|");
    hasher.update(range_spec.as_bytes());
    hex::encode(hasher.finalize())
}
