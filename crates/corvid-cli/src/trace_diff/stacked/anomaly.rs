//! Stack receipt anomaly schema and fingerprinting.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Typed anomaly surfaced by the composer. Cache-integrity is the
/// only class intended to hard-fail; every other class surfaces in
/// the receipt with a non-zero policy exit so reviewers can see
/// exactly what tripped.
#[derive(Debug, Clone, Serialize)]
pub(in crate::trace_diff) struct Anomaly {
    pub class: AnomalyClass,
    pub severity: AnomalySeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introduced_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_family: Option<String>,
    pub affected_deltas: Vec<String>,
    /// Structured reason string. LLM `narrative` + `remediation`
    /// remain `None` at compose time; the LLM-surface commit of
    /// this slice populates them.
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    /// Canonical sha256 over structured fields. GitLab /
    /// CodeClimate renderers dedupe by this across re-runs of the
    /// same stack — same inputs → same fingerprint → no phantom
    /// "new" findings on every re-run.
    pub fingerprint: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(in crate::trace_diff) enum AnomalyClass {
    /// Per-commit receipt or envelope hash mismatched on
    /// retrieval from the content-addressed cache. Security
    /// signal — consumers never proceed past this. (Produced by
    /// the cache-integration commit, not this composer commit;
    /// variant reserved here for schema stability.)
    CacheIntegrity,
    /// Two adjacent Class B transitions on the same entity where
    /// the first's `to` ≠ the second's `from`. Usually indicates
    /// a rebase artifact, a cherry-pick that scrambled
    /// intermediates, or an algebra bug in per-commit delta
    /// emission.
    AlgebraicChainBreak,
    /// Two Class A lifecycle deltas in the same direction
    /// (`added` then `added`, `gained` then `gained`, etc.)
    /// without an intervening inverse. Well-formed git history
    /// shouldn't produce this.
    SameDirectionDuplicate,
    /// Same-agent delta applied out of semantic order (e.g.
    /// `dangerous_gained:X` before `added:X`). Reserved — the
    /// composer doesn't enforce base-state ordering in this
    /// commit; the later ordering-check commit populates it.
    OrderingViolation,
    /// A delta references an agent not present in the commit's
    /// ABI, or a trace exercises agents not in any waypoint.
    /// Reserved — metadata-drift checks land with replay
    /// integration.
    CrossReferenceDrift,
    /// Algebra predicted no behavior change on a (trace, commit)
    /// pair; replay showed divergence. Reserved — populated by
    /// the predictive-replay commit.
    PredictionMismatch,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(in crate::trace_diff) enum AnomalySeverity {
    /// Composer refuses to proceed. Only cache-integrity uses
    /// this today.
    HardFail,
    /// Surfaces in the receipt with non-zero policy exit; receipt
    /// still emits so reviewers can see it.
    Surface,
}

pub(in crate::trace_diff) fn build_anomaly(
    class: AnomalyClass,
    severity: AnomalySeverity,
    introduced_at: Option<String>,
    affected_agent: Option<String>,
    affected_family: Option<String>,
    affected_deltas: Vec<String>,
    detail: String,
) -> Anomaly {
    let mut hasher = Sha256::new();
    hasher.update(format!("{:?}", class).as_bytes());
    hasher.update(b"|");
    hasher.update(introduced_at.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(affected_agent.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(affected_family.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"|");
    for d in &affected_deltas {
        hasher.update(d.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"|");
    hasher.update(detail.as_bytes());
    let fingerprint = hex::encode(hasher.finalize());

    Anomaly {
        class,
        severity,
        introduced_at,
        affected_agent,
        affected_family,
        affected_deltas,
        detail,
        narrative: None,
        remediation: None,
        fingerprint,
    }
}
