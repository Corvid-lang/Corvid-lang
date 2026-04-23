//! Per-trace attribution engine for stack receipts — the evidence
//! model that turns `"this stack caused this regression"` into a
//! machine-checkable, cryptographically addressable claim.
//!
//! An [`Attribution`] records, per trace:
//!
//! - *which commit in the stack caused the trace to newly diverge*
//!   (`diverged_at`: the first waypoint where the trace's output
//!   differed from base), and
//! - *which deltas in that commit are plausible causes*
//!   (`candidate_deltas`: full delta set at commit-level today;
//!   minimal causal subset via delta-ddmin isolation replay in a
//!   later commit of the slice), and
//! - *a content-addressed reproducibility envelope* an auditor can
//!   fetch by hash, replay locally, and confirm the attribution
//!   without trusting the deployer.
//!
//! This commit ships the data model + canonical hashing only. The
//! population path (running counterfactual replay per waypoint,
//! identifying `diverged_at`, emitting attributions into
//! [`StackReceipt`]) lands in the next commit when the step-2/N
//! `--stack --traces` ban is lifted. The delta-level isolation
//! replay + ddmin for minimal causal sets ships as a follow-up
//! once commit-level attribution is stable.
//!
//! Schema stability: every field the later commits need is already
//! shaped here so the JSON surface doesn't churn — `candidate_deltas`
//! grows more precise without changing type; `responsible_commit`
//! can diverge from `diverged_at` when delta-isolation shows a
//! later commit composed with earlier ones to produce the
//! divergence.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Per-trace attribution: what in the stack caused a recorded trace
/// to newly diverge, plus the reproducibility evidence for that
/// claim. Populated when `--stack --traces <dir>` is set; the
/// `attributions` field on [`super::stacked::StackReceipt`] stays
/// empty otherwise.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Attribution {
    /// Stable identifier for the trace, derived from the trace
    /// file's bytes so two analyses of the same trace attribute
    /// against the same key. Content-addressed: same trace bytes
    /// → same id across machines.
    pub trace_id: String,
    /// First commit in the stack where this trace's output
    /// differed from its base output. `None` means the trace
    /// passed through the entire stack unchanged — no divergence
    /// to attribute.
    pub diverged_at: Option<String>,
    /// Commit SHA the attribution engine judges as the cause. For
    /// commit-level attribution this is the same as `diverged_at`.
    /// Delta-level follow-up may report a commit different from
    /// `diverged_at` when a later commit composed with earlier
    /// ones to produce the divergence — in that case
    /// `responsible_commit` points at the earlier enabler, not
    /// the commit where divergence first appeared.
    pub responsible_commit: Option<String>,
    /// Delta keys from the responsible commit judged as plausible
    /// causes. Commit-level attribution populates this with the
    /// full delta set of `responsible_commit`. Delta-level
    /// isolation + ddmin narrows to the minimal causal subset in
    /// a later commit of the slice.
    pub candidate_deltas: Vec<String>,
    /// sha256 of the canonical JSON of the corresponding
    /// [`ReproducibilityEnvelope`]. Auditors fetch the envelope
    /// by this hash from the content-addressed cache, replay
    /// locally, confirm the attribution without trusting the
    /// deployer. Absent attributions with no divergence omit the
    /// envelope entirely; this hash stays populated only when
    /// there's a divergence worth reproducing.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reproducibility_envelope_hash: String,
}

/// Reproducibility envelope: the evidence package for one
/// [`Attribution`]. Content-addressed; same inputs → same hash.
/// Auditor flow: fetch by hash, re-run counterfactual replay
/// locally with the same trace + stack, observe whether the
/// recorded waypoint result hashes match. Divergent hashes prove
/// tampering or non-determinism; matching hashes confirm the
/// claim.
///
/// The envelope does NOT embed large artifacts (trace bytes,
/// source files) — those are referenced by their own content
/// addresses. Auditors bring the trace corpus; the stack
/// receipt's `components` list provides the per-commit receipt
/// hashes. The envelope records *what the replay produced* at
/// each waypoint so re-replay is a direct byte-comparison.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ReproducibilityEnvelope {
    /// Trace identifier this envelope attributes for. Matches
    /// the owning `Attribution.trace_id`.
    pub trace_id: String,
    /// Per-commit replay outcome hashes, starting with base
    /// (`waypoints[0]`) then each commit in stack order. Index 0
    /// is always the baseline against which `diverged_from_base`
    /// is measured.
    pub waypoints: Vec<Waypoint>,
    /// Keyid of the signature over the canonical JSON of this
    /// envelope, once step 6/N Merkle signing wires it up.
    /// Absent on unsigned envelopes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_keyid: Option<String>,
}

/// One waypoint in a [`ReproducibilityEnvelope`]. Records the
/// commit SHA and the sha256 of the canonical replay output at
/// that waypoint. Byte-comparing these hashes is how an auditor
/// re-verifies the attribution.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Waypoint {
    /// Commit SHA. For index 0 this is the stack base; for index
    /// i > 0 it's the i-th commit in the stack's `components`
    /// list.
    pub commit_sha: String,
    /// sha256 of the canonical replay result at this waypoint.
    /// Populated by the counterfactual-replay integration that
    /// lands in the next commit of this step.
    pub result_hash: String,
    /// True when `result_hash != waypoints[0].result_hash`. Makes
    /// the `diverged_at` computation a one-pass scan.
    pub diverged_from_base: bool,
}

/// Canonical sha256 of a trace's bytes — the trace_id used
/// throughout attribution. Content-addressed: same bytes → same
/// id on any machine, so auditors attribute against the same
/// keys the recording author did.
pub(super) fn trace_id_from_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Canonical content hash of a [`ReproducibilityEnvelope`]. The
/// value becomes `Attribution.reproducibility_envelope_hash` and
/// is how the envelope is looked up in the content-addressed
/// cache. sha256 over serde-json canonical serialization — stable
/// across platforms because the types define Serialize via serde
/// derive (field order + formatting are deterministic).
pub(super) fn envelope_hash(envelope: &ReproducibilityEnvelope) -> String {
    let canonical = serde_json::to_string(envelope)
        .expect("ReproducibilityEnvelope is trivially serializable");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_envelope(trace_id: &str) -> ReproducibilityEnvelope {
        ReproducibilityEnvelope {
            trace_id: trace_id.to_string(),
            waypoints: vec![],
            signature_keyid: None,
        }
    }

    #[test]
    fn trace_id_is_sha256_hex() {
        let id = trace_id_from_bytes(b"some trace bytes");
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn trace_id_is_content_addressed() {
        let a = trace_id_from_bytes(b"trace one");
        let b = trace_id_from_bytes(b"trace one");
        let c = trace_id_from_bytes(b"trace two");
        assert_eq!(a, b, "same bytes must produce same id");
        assert_ne!(a, c, "different bytes must produce different id");
    }

    #[test]
    fn envelope_hash_is_deterministic() {
        let e = ReproducibilityEnvelope {
            trace_id: "t1".into(),
            waypoints: vec![
                Waypoint {
                    commit_sha: "c1".into(),
                    result_hash: "r1".into(),
                    diverged_from_base: false,
                },
                Waypoint {
                    commit_sha: "c2".into(),
                    result_hash: "r2".into(),
                    diverged_from_base: true,
                },
            ],
            signature_keyid: None,
        };
        let h1 = envelope_hash(&e);
        let h2 = envelope_hash(&e);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn envelope_hash_differs_by_waypoint_content() {
        let base = empty_envelope("t1");
        let with_waypoint = ReproducibilityEnvelope {
            waypoints: vec![Waypoint {
                commit_sha: "c1".into(),
                result_hash: "r1".into(),
                diverged_from_base: false,
            }],
            ..base.clone()
        };
        assert_ne!(envelope_hash(&base), envelope_hash(&with_waypoint));
    }

    #[test]
    fn envelope_hash_differs_by_signature_presence() {
        let unsigned = empty_envelope("t1");
        let signed = ReproducibilityEnvelope {
            signature_keyid: Some("corvid-ci-main".into()),
            ..unsigned.clone()
        };
        assert_ne!(envelope_hash(&unsigned), envelope_hash(&signed));
    }

    #[test]
    fn attribution_serializes_without_envelope_hash_when_empty() {
        // When a trace didn't diverge, no reproducibility envelope
        // exists; the JSON surface shouldn't carry an empty hash
        // field that misleads consumers.
        let a = Attribution {
            trace_id: "t1".into(),
            diverged_at: None,
            responsible_commit: None,
            candidate_deltas: vec![],
            reproducibility_envelope_hash: String::new(),
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            !json.contains("reproducibility_envelope_hash"),
            "empty hash should be skipped; got {json}"
        );
    }

    #[test]
    fn attribution_serializes_with_envelope_hash_when_populated() {
        let a = Attribution {
            trace_id: "t1".into(),
            diverged_at: Some("c3".into()),
            responsible_commit: Some("c3".into()),
            candidate_deltas: vec!["agent.added:foo".into()],
            reproducibility_envelope_hash: "a".repeat(64),
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("reproducibility_envelope_hash"));
        assert!(json.contains(&"a".repeat(64)));
    }
}
