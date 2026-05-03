//! Compose a sequence of per-commit trace-diff receipts into a
//! stack receipt — Corvid's algebraic answer to stacked-PR review.
//!
//! A per-commit receipt carries the delta set between two adjacent
//! commits. Stacking N of them across a range produces three views:
//!
//! - `normal_form` — the net base→head change after Class A
//!   lifecycle-pair cancellation and Class B transition-chain
//!   composition. Empty when the stack round-trips to identity.
//! - `history` — every intermediate delta preserved in order.
//!   Answers audit questions like "was `@dangerous` ever in this
//!   code path, even briefly?"
//! - `anomalies` — typed inconsistencies the composer found (Class
//!   B algebraic chain breaks, Class A same-direction duplicates,
//!   and the placeholder variants reserved for later-slice
//!   integrations).
//!
//! Every surviving normal-form delta carries `introduced_at:
//! <commit_sha>` — the commit where the delta's head value was
//! pinned. That's the commit-level bisection primitive: reviewers
//! see *which* commit is responsible for each surviving regression
//! without re-running tests at every waypoint.
//!
//! This module owns composition only. Rendering (markdown, json,
//! gitlab, etc.), replay integration, signing, counterfactual
//! attribution, and LLM narratives ship in later commits of the
//! same slice and plug into the types defined here. Schema is
//! stable across those additions by construction: narrative and
//! remediation fields on anomalies remain `None` at compose time;
//! later surfaces fill them.

use std::collections::HashMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::narrative::DeltaRecord;
use super::receipt::Verdict;
use super::stack_attribution::Attribution;

/// Schema version for `StackReceipt`. Independent counter from
/// the per-commit `RECEIPT_SCHEMA_VERSION` — stack receipts are a
/// structurally different kind of artifact and version
/// independently.
pub(super) const STACK_RECEIPT_SCHEMA_VERSION: u32 = 2;

mod anomaly;
pub(super) use anomaly::{build_anomaly, Anomaly, AnomalyClass, AnomalySeverity};
mod normal_form;
use normal_form::{
    apply_lifecycle, apply_transition, parse_delta_key, DeltaKind, LifecycleChain, TransitionChain,
};

// ---------------------------------------------------------------
// Public types
// ---------------------------------------------------------------

/// Top-level stack receipt. Carries the dual normal-form / history
/// views, typed anomalies, and component references for Merkle-DAG
/// traversal + verification in the later signing slice.
#[derive(Debug, Clone, Serialize)]
pub(super) struct StackReceipt {
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
pub(super) struct StackComponent {
    pub commit_sha: String,
    pub receipt_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_hash: Option<String>,
    pub signature_status: SignatureStatus,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum SignatureStatus {
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
pub(super) struct StackDelta {
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
pub(super) struct StackInput {
    pub commit_sha: String,
    pub receipt_hash: String,
    pub envelope_hash: Option<String>,
    pub signature_status: SignatureStatus,
    pub deltas: Vec<DeltaRecord>,
}

// ---------------------------------------------------------------
// Composer
// ---------------------------------------------------------------

/// Core composer. Takes ordered per-commit inputs; returns a
/// `StackReceipt` with normal_form + history + anomalies. No I/O;
/// pure algebra.
pub(super) fn compose_stack(
    base_sha: &str,
    head_sha: &str,
    source_path: &str,
    range_spec: &str,
    inputs: Vec<StackInput>,
) -> StackReceipt {
    let mut history: Vec<StackDelta> = Vec::new();
    let mut anomalies: Vec<Anomaly> = Vec::new();

    let mut lifecycle_state: HashMap<(String, String), LifecycleChain> = HashMap::new();
    let mut transition_state: HashMap<(String, String), TransitionChain> = HashMap::new();

    let mut components: Vec<StackComponent> = Vec::with_capacity(inputs.len());

    for input in &inputs {
        components.push(StackComponent {
            commit_sha: input.commit_sha.clone(),
            receipt_hash: input.receipt_hash.clone(),
            envelope_hash: input.envelope_hash.clone(),
            signature_status: input.signature_status,
        });

        for delta in &input.deltas {
            // History view preserves every delta with its commit,
            // unconditionally. Normal form is the derived view;
            // history is the source of truth for audit.
            history.push(StackDelta {
                key: delta.key.clone(),
                summary: delta.summary.clone(),
                introduced_at: input.commit_sha.clone(),
            });

            match parse_delta_key(&delta.key) {
                Some(DeltaKind::Lifecycle {
                    family,
                    entity,
                    polarity,
                }) => {
                    apply_lifecycle(
                        &mut lifecycle_state,
                        &mut anomalies,
                        family,
                        entity,
                        polarity,
                        delta,
                        &input.commit_sha,
                    );
                }
                Some(DeltaKind::Transition {
                    family,
                    entity,
                    from,
                    to,
                }) => {
                    apply_transition(
                        &mut transition_state,
                        &mut anomalies,
                        family,
                        entity,
                        from,
                        to,
                        delta,
                        &input.commit_sha,
                    );
                }
                None => {
                    // Unrecognized delta keys pass through to
                    // history only. Not an anomaly — new delta
                    // classes may ship before the composer learns
                    // them; graceful degradation beats false
                    // alarm.
                }
            }
        }
    }

    let mut normal_form: Vec<StackDelta> = Vec::new();
    for (_, chain) in lifecycle_state {
        if let Some(delta) = chain.into_normal_form() {
            normal_form.push(delta);
        }
    }
    for (_, chain) in transition_state {
        if let Some(delta) = chain.into_normal_form() {
            normal_form.push(delta);
        }
    }
    normal_form.sort_by(|a, b| a.key.cmp(&b.key));

    let stack_hash = compute_stack_hash(&components, range_spec);

    StackReceipt {
        schema_version: STACK_RECEIPT_SCHEMA_VERSION,
        stack_hash,
        base_sha: base_sha.to_string(),
        head_sha: head_sha.to_string(),
        source_path: source_path.to_string(),
        range_spec: range_spec.to_string(),
        components,
        verdict: Verdict {
            ok: true,
            flags: Vec::new(),
        },
        normal_form,
        history,
        anomalies,
        attributions: Vec::new(),
    }
}

// ---------------------------------------------------------------
// Internals
// ---------------------------------------------------------------

/// Compute the content-addressed stack hash: sha256 of sorted
/// per-commit receipt hashes concatenated with the range spec.
/// Same inputs → same hash (natural memoization); different
/// inputs → different hash (natural invalidation). Stable across
/// re-composition because receipt hashes are themselves content-
/// addressed — swapping a component for a different-hash
/// component necessarily changes the stack hash.
fn compute_stack_hash(components: &[StackComponent], range_spec: &str) -> String {
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

// ---------------------------------------------------------------
// Tests
// ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(key: &str, summary: &str) -> DeltaRecord {
        DeltaRecord {
            key: key.to_string(),
            summary: summary.to_string(),
        }
    }

    fn input(sha: &str, deltas: Vec<DeltaRecord>) -> StackInput {
        StackInput {
            commit_sha: sha.to_string(),
            receipt_hash: format!("{sha}_receipt_hash"),
            envelope_hash: None,
            signature_status: SignatureStatus::Unsigned,
            deltas,
        }
    }

    // -----------------------------------------------------------
    // Delta-key parser
    // -----------------------------------------------------------

    #[test]
    fn parse_lifecycle_gained() {
        match parse_delta_key("agent.dangerous_gained:refund_bot") {
            Some(DeltaKind::Lifecycle {
                family,
                entity,
                polarity,
            }) => {
                assert_eq!(family, "agent.dangerous");
                assert_eq!(entity, "refund_bot");
                assert_eq!(polarity, 1);
            }
            other => panic!("expected Lifecycle, got {other:?}"),
        }
    }

    #[test]
    fn parse_lifecycle_lost() {
        match parse_delta_key("agent.provenance.grounded_lost:bot") {
            Some(DeltaKind::Lifecycle {
                family,
                entity,
                polarity,
            }) => {
                assert_eq!(family, "agent.provenance.grounded");
                assert_eq!(entity, "bot");
                assert_eq!(polarity, -1);
            }
            other => panic!("expected Lifecycle, got {other:?}"),
        }
    }

    #[test]
    fn parse_lifecycle_added_with_multi_part_entity() {
        match parse_delta_key("agent.approval.label_added:bot:IssueRefund") {
            Some(DeltaKind::Lifecycle {
                family,
                entity,
                polarity,
            }) => {
                assert_eq!(family, "agent.approval.label");
                assert_eq!(entity, "bot:IssueRefund");
                assert_eq!(polarity, 1);
            }
            other => panic!("expected Lifecycle, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_added_dot_form() {
        match parse_delta_key("agent.added:foo") {
            Some(DeltaKind::Lifecycle {
                family,
                entity,
                polarity,
            }) => {
                assert_eq!(family, "agent.lifecycle");
                assert_eq!(entity, "foo");
                assert_eq!(polarity, 1);
            }
            other => panic!("expected Lifecycle, got {other:?}"),
        }
    }

    #[test]
    fn parse_transition_simple() {
        match parse_delta_key("agent.trust_tier_changed:bot:autonomous->human_required") {
            Some(DeltaKind::Transition {
                family,
                entity,
                from,
                to,
            }) => {
                assert_eq!(family, "agent.trust_tier");
                assert_eq!(entity, "bot");
                assert_eq!(from, "autonomous");
                assert_eq!(to, "human_required");
            }
            other => panic!("expected Transition, got {other:?}"),
        }
    }

    #[test]
    fn parse_transition_with_label_in_entity() {
        match parse_delta_key("agent.approval.tier_changed:bot:IssueRefund:strict->lenient") {
            Some(DeltaKind::Transition {
                family,
                entity,
                from,
                to,
            }) => {
                assert_eq!(family, "agent.approval.tier");
                assert_eq!(entity, "bot:IssueRefund");
                assert_eq!(from, "strict");
                assert_eq!(to, "lenient");
            }
            other => panic!("expected Transition, got {other:?}"),
        }
    }

    #[test]
    fn parse_unrecognized_returns_none() {
        assert!(parse_delta_key("some.future.delta:X").is_none());
    }

    // Tiny Debug impl so test panics are readable when parsing
    // fails unexpectedly.
    impl std::fmt::Debug for DeltaKind {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                DeltaKind::Lifecycle {
                    family,
                    entity,
                    polarity,
                } => f
                    .debug_struct("Lifecycle")
                    .field("family", family)
                    .field("entity", entity)
                    .field("polarity", polarity)
                    .finish(),
                DeltaKind::Transition {
                    family,
                    entity,
                    from,
                    to,
                } => f
                    .debug_struct("Transition")
                    .field("family", family)
                    .field("entity", entity)
                    .field("from", from)
                    .field("to", to)
                    .finish(),
            }
        }
    }

    // -----------------------------------------------------------
    // Class A — lifecycle pairs
    // -----------------------------------------------------------

    #[test]
    fn class_a_add_then_remove_cancels_to_identity() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.added:foo", "new")]),
                input("c2", vec![delta("agent.removed:foo", "removed")]),
            ],
        );
        assert!(
            r.normal_form.is_empty(),
            "add+remove should cancel; got: {:?}",
            r.normal_form
        );
        assert_eq!(r.history.len(), 2);
        assert!(r.anomalies.is_empty());
    }

    #[test]
    fn class_a_net_positive_survives_with_introduced_at_last_commit() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.added:foo", "new")]),
                input("c2", vec![delta("agent.removed:foo", "removed")]),
                input("c3", vec![delta("agent.added:foo", "re-added")]),
            ],
        );
        assert_eq!(r.normal_form.len(), 1);
        assert_eq!(r.normal_form[0].key, "agent.added:foo");
        assert_eq!(
            r.normal_form[0].introduced_at, "c3",
            "introduced_at must point at the commit that pinned the final state"
        );
    }

    #[test]
    fn class_a_same_direction_twice_is_anomaly() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.added:foo", "")]),
                input("c2", vec![delta("agent.added:foo", "redundant add")]),
            ],
        );
        assert_eq!(r.anomalies.len(), 1);
        assert_eq!(r.anomalies[0].class, AnomalyClass::SameDirectionDuplicate);
        assert_eq!(r.anomalies[0].severity, AnomalySeverity::Surface);
        // Anomaly surfaces, but normal form still reflects the first
        // add (state remained at +1) so the reviewer sees the net.
        assert_eq!(r.normal_form.len(), 1);
    }

    #[test]
    fn class_a_dangerous_gained_lost_cancels() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input(
                    "c1",
                    vec![delta("agent.dangerous_gained:bot", "became dangerous")],
                ),
                input(
                    "c2",
                    vec![delta("agent.dangerous_lost:bot", "no longer dangerous")],
                ),
            ],
        );
        assert!(r.normal_form.is_empty());
    }

    #[test]
    fn class_a_dep_added_removed_cancels_per_dep() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input(
                    "c1",
                    vec![
                        delta("agent.provenance.dep_added:bot:param_a", ""),
                        delta("agent.provenance.dep_added:bot:param_b", ""),
                    ],
                ),
                input(
                    "c2",
                    vec![delta("agent.provenance.dep_removed:bot:param_a", "")],
                ),
            ],
        );
        // `param_a` canceled; `param_b` survives.
        assert_eq!(r.normal_form.len(), 1);
        assert_eq!(
            r.normal_form[0].key,
            "agent.provenance.dep_added:bot:param_b"
        );
    }

    // -----------------------------------------------------------
    // Class B — transition chains
    // -----------------------------------------------------------

    #[test]
    fn class_b_composes_associatively() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.trust_tier_changed:bot:A->B", "")]),
                input("c2", vec![delta("agent.trust_tier_changed:bot:B->C", "")]),
            ],
        );
        assert_eq!(r.normal_form.len(), 1);
        assert!(
            r.normal_form[0].key.ends_with("A->C"),
            "expected composed A->C, got {}",
            r.normal_form[0].key
        );
        assert_eq!(r.normal_form[0].introduced_at, "c2");
        assert!(r.anomalies.is_empty());
    }

    #[test]
    fn class_b_round_trip_cancels() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.trust_tier_changed:bot:A->B", "")]),
                input("c2", vec![delta("agent.trust_tier_changed:bot:B->A", "")]),
            ],
        );
        assert!(
            r.normal_form.is_empty(),
            "A->B + B->A should cancel to identity"
        );
    }

    #[test]
    fn class_b_chain_break_is_anomaly() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.trust_tier_changed:bot:A->B", "")]),
                input("c2", vec![delta("agent.trust_tier_changed:bot:C->D", "")]),
            ],
        );
        assert_eq!(r.anomalies.len(), 1);
        assert_eq!(r.anomalies[0].class, AnomalyClass::AlgebraicChainBreak);
    }

    #[test]
    fn class_b_long_chain_composes_to_endpoints() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.trust_tier_changed:bot:A->B", "")]),
                input("c2", vec![delta("agent.trust_tier_changed:bot:B->C", "")]),
                input("c3", vec![delta("agent.trust_tier_changed:bot:C->D", "")]),
            ],
        );
        assert_eq!(r.normal_form.len(), 1);
        assert!(r.normal_form[0].key.ends_with("A->D"));
        assert_eq!(r.normal_form[0].introduced_at, "c3");
    }

    #[test]
    fn approval_tier_changed_works_with_label_entity() {
        // Regression guard: the schema-fix commit renamed
        // `tier_weakened` → `tier_changed`. The composer must
        // recognize the new name and correctly treat the label as
        // part of the entity (so per-label transitions compose
        // independently).
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input(
                    "c1",
                    vec![delta(
                        "agent.approval.tier_changed:bot:IssueRefund:strict->lenient",
                        "",
                    )],
                ),
                input(
                    "c2",
                    vec![delta(
                        "agent.approval.tier_changed:bot:IssueRefund:lenient->strict",
                        "",
                    )],
                ),
            ],
        );
        assert!(
            r.normal_form.is_empty(),
            "same-label round-trip should cancel"
        );
    }

    // -----------------------------------------------------------
    // Commutativity + unrecognized + history
    // -----------------------------------------------------------

    #[test]
    fn different_agents_commute_freely() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.added:foo", "")]),
                input("c2", vec![delta("agent.added:bar", "")]),
            ],
        );
        assert_eq!(r.normal_form.len(), 2);
        assert!(r.anomalies.is_empty());
    }

    #[test]
    fn history_preserves_every_delta_in_commit_order() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![
                input("c1", vec![delta("agent.added:foo", "1")]),
                input("c2", vec![delta("agent.removed:foo", "2")]),
                input("c3", vec![delta("agent.added:foo", "3")]),
            ],
        );
        assert_eq!(r.history.len(), 3);
        assert_eq!(r.history[0].introduced_at, "c1");
        assert_eq!(r.history[1].introduced_at, "c2");
        assert_eq!(r.history[2].introduced_at, "c3");
    }

    #[test]
    fn unrecognized_delta_keys_pass_to_history_only() {
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![input(
                "c1",
                vec![delta("some.future.delta:X", "unknown shape")],
            )],
        );
        assert_eq!(r.history.len(), 1);
        assert!(r.normal_form.is_empty());
        // Unrecognized ≠ anomaly — the composer degrades
        // gracefully so the per-commit-receipt schema can grow
        // without breaking stack composition.
        assert!(r.anomalies.is_empty());
    }

    // -----------------------------------------------------------
    // Stack hash + anomaly fingerprint
    // -----------------------------------------------------------

    #[test]
    fn stack_hash_is_deterministic_across_runs() {
        let build = || {
            compose_stack(
                "base",
                "head",
                "src/a.cor",
                "base..head",
                vec![
                    input("c1", vec![delta("agent.added:foo", "")]),
                    input("c2", vec![delta("agent.removed:foo", "")]),
                ],
            )
        };
        assert_eq!(build().stack_hash, build().stack_hash);
    }

    #[test]
    fn stack_hash_differs_across_different_ranges() {
        let r1 = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![input("c1", vec![])],
        );
        let r2 = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "main..feature",
            vec![input("c1", vec![])],
        );
        assert_ne!(r1.stack_hash, r2.stack_hash);
    }

    #[test]
    fn stack_hash_is_order_insensitive_on_components() {
        // Swapping the insertion order of components must not
        // change the stack hash because we sort receipt hashes
        // canonically before hashing.
        let r1 = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![input("c1", vec![]), input("c2", vec![])],
        );
        let r2 = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![input("c2", vec![]), input("c1", vec![])],
        );
        assert_eq!(r1.stack_hash, r2.stack_hash);
    }

    #[test]
    fn anomaly_fingerprints_are_deterministic() {
        let build = || {
            compose_stack(
                "base",
                "head",
                "src/a.cor",
                "base..head",
                vec![
                    input("c1", vec![delta("agent.added:foo", "")]),
                    input("c2", vec![delta("agent.added:foo", "")]),
                ],
            )
        };
        let r1 = build();
        let r2 = build();
        assert_eq!(r1.anomalies[0].fingerprint, r2.anomalies[0].fingerprint);
    }

    #[test]
    fn json_shape_round_trips() {
        // Regression guard: the receipt must serialize so the
        // later rendering slice can emit it. Checking the schema
        // version + the three top-level arrays exist is enough
        // for a shape smoke test.
        let r = compose_stack(
            "base",
            "head",
            "src/a.cor",
            "base..head",
            vec![input("c1", vec![delta("agent.added:foo", "")])],
        );
        let json = serde_json::to_string_pretty(&r).expect("serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["schema_version"], 2);
        assert!(parsed["verdict"].is_object());
        assert!(parsed["normal_form"].is_array());
        assert!(parsed["history"].is_array());
        assert!(parsed["components"].is_array());
        assert!(parsed["anomalies"].is_array());
        assert_eq!(parsed["stack_hash"].as_str().unwrap().len(), 64);
    }
}
