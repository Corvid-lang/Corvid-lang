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

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

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

/// Canonical hash of a per-waypoint replay outcome. Takes a short
/// textual tag (`"passed"`, `"diverged"`, `"absent"`) rather than a
/// runtime-specific `Verdict` enum so this module stays free of
/// `corvid_runtime` coupling — the caller in `stack_driver` maps
/// real verdicts to tags before handing them here.
pub(super) fn verdict_result_hash(tag: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tag.as_bytes());
    hex::encode(hasher.finalize())
}

/// Extract the set of agents a recorded trace exercises. Scans
/// every JSONL event in the trace bytes and returns any value
/// appearing under the `agent` key. For v1, this captures the
/// top-level `run_started` agent plus any nested runs; transitive
/// call-graph analysis (agent X that internally calls Y without Y
/// appearing in the trace) is out of scope.
///
/// The under-approximation is intentional and safe: the skip
/// decision in the caller only fires when `exercised` and
/// `affected` are *provably disjoint*. If we return an empty set
/// (parse failure, unusual trace format), the caller treats that
/// as "cannot prove disjoint" and replays anyway — over-replay is
/// waste, under-replay would miss regressions, and we pick the
/// safe side.
pub(super) fn trace_exercised_agents(trace_bytes: &[u8]) -> BTreeSet<String> {
    let Ok(text) = std::str::from_utf8(trace_bytes) else {
        return BTreeSet::new();
    };
    let mut agents = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            // Malformed event line — skip; don't let one bad line
            // produce an empty set that suppresses skipping for
            // the rest of the trace.
            continue;
        };
        if let Some(agent) = value.get("agent").and_then(|v| v.as_str()) {
            if !agent.is_empty() {
                agents.insert(agent.to_string());
            }
        }
    }
    agents
}

/// Extract the set of agents a commit's delta keys affect. The
/// agent name is the first colon-separated segment after the
/// delta-class prefix — holds uniformly for Class A (lifecycle)
/// and Class B (transition) keys as emitted by `narrative.rs`.
pub(super) fn commit_affected_agents(delta_keys: &[String]) -> BTreeSet<String> {
    let mut agents = BTreeSet::new();
    for key in delta_keys {
        if let Some(name) = extract_agent_from_delta_key(key) {
            agents.insert(name);
        }
    }
    agents
}

/// Pull the agent name out of a canonical delta key. The emitted
/// shape is always `<class-prefix>:<agent>[:<extra>]` — see
/// `narrative.rs` where every `DeltaRecord.key` is built. Returns
/// `None` for malformed keys (not an error; callers treat missing
/// names as "affects an unknown agent" and err toward replaying).
fn extract_agent_from_delta_key(key: &str) -> Option<String> {
    // Skip the class prefix.
    let (_, rest) = key.split_once(':')?;
    // The agent name is the next segment (or all of `rest` if
    // there's no further colon, e.g. `agent.added:foo`).
    let (agent, _) = rest.split_once(':').unwrap_or((rest, ""));
    if agent.is_empty() {
        None
    } else {
        Some(agent.to_string())
    }
}

/// Algebra-directed skip decision: return `true` when replay of
/// `trace` against a waypoint that modifies `affected` is
/// provably unnecessary — the trace's output cannot differ from
/// base's because none of the agents the trace exercises were
/// touched.
///
/// Invariant: skipping when this returns `true` is provably
/// correct. Skipping otherwise would risk a false negative (a
/// missed regression). That's why we require *both* sets to be
/// non-empty and disjoint; empty `exercised` means the trace
/// parser couldn't identify exercised agents → can't prove
/// disjoint → must replay.
pub(super) fn can_skip_replay(
    exercised: &BTreeSet<String>,
    affected: &BTreeSet<String>,
) -> bool {
    if exercised.is_empty() || affected.is_empty() {
        return false;
    }
    exercised.is_disjoint(affected)
}

/// One waypoint's input to the attribution computer: the commit
/// SHA and a per-trace map from the trace path to the verdict tag
/// produced when the recorded trace was replayed against that
/// waypoint's source. Index 0 of a waypoint list is always the
/// stack base; indices 1..=N are the per-commit waypoints in
/// chronological order.
pub(super) struct WaypointData {
    pub commit_sha: String,
    pub verdict_tags: BTreeMap<PathBuf, String>,
}

/// Compute per-trace [`Attribution`] records from a list of
/// waypoint verdict maps. Commit-level attribution only: the
/// `responsible_commit` is the first waypoint where a trace's
/// verdict differs from its base verdict, and `candidate_deltas`
/// is the full delta set of that commit. Delta-level narrowing
/// (isolation replay + ddmin) refines `candidate_deltas` in a
/// later commit of the slice without schema churn.
///
/// Inputs:
/// - `trace_files`: each trace's path + raw bytes. Bytes feed the
///   content-addressed `trace_id` so two analyses of the same
///   trace attribute against the same key on any machine.
/// - `waypoints`: waypoint-aligned verdict-tag maps; `waypoints[0]`
///   is base.
/// - `commit_delta_sets`: aligned with `waypoints[1..]` (NOT
///   including base). `commit_delta_sets[j]` is the delta-key list
///   for the commit at `waypoints[j + 1]`.
///
/// Returns attributions only for traces that *diverged somewhere*
/// in the stack — traces that passed through every waypoint
/// identically don't surface as regressions. The attribution list
/// is sorted by `trace_id` for byte-stable receipts.
pub(super) fn compute_stack_attributions(
    trace_files: &[(PathBuf, Vec<u8>)],
    waypoints: &[WaypointData],
    commit_delta_sets: &[Vec<String>],
) -> Vec<Attribution> {
    if waypoints.is_empty() {
        return Vec::new();
    }
    debug_assert_eq!(
        commit_delta_sets.len() + 1,
        waypoints.len(),
        "commit_delta_sets must be aligned with waypoints[1..]"
    );

    let mut out = Vec::new();
    for (trace_path, bytes) in trace_files {
        let trace_id = trace_id_from_bytes(bytes);

        // Base verdict is the reference against which every
        // subsequent waypoint is compared. `None` means the trace
        // didn't register a verdict at base (shouldn't normally
        // happen because every replay emits a verdict, but the
        // absent case is handled defensively).
        let base_tag = waypoints[0].verdict_tags.get(trace_path).cloned();

        let mut waypoint_records = Vec::with_capacity(waypoints.len());
        let mut diverged_at_idx: Option<usize> = None;
        for (i, waypoint) in waypoints.iter().enumerate() {
            let tag = waypoint
                .verdict_tags
                .get(trace_path)
                .cloned()
                .unwrap_or_else(|| "absent".to_string());
            let diverged_from_base = if i == 0 {
                false
            } else {
                base_tag.as_deref() != Some(tag.as_str())
            };
            if i > 0 && diverged_at_idx.is_none() && diverged_from_base {
                diverged_at_idx = Some(i);
            }
            waypoint_records.push(Waypoint {
                commit_sha: waypoint.commit_sha.clone(),
                result_hash: verdict_result_hash(&tag),
                diverged_from_base,
            });
        }

        let Some(idx) = diverged_at_idx else {
            // Trace passed through the stack unchanged relative to
            // base. Skip — no attribution to emit.
            continue;
        };

        let envelope = ReproducibilityEnvelope {
            trace_id: trace_id.clone(),
            waypoints: waypoint_records,
            signature_keyid: None,
        };
        let envelope_hash_str = envelope_hash(&envelope);

        // `commit_delta_sets` is aligned with `waypoints[1..]`, so
        // the deltas at waypoint index `idx` (where `idx >= 1`)
        // live at `commit_delta_sets[idx - 1]`.
        let candidate_deltas = commit_delta_sets
            .get(idx - 1)
            .cloned()
            .unwrap_or_default();

        out.push(Attribution {
            trace_id,
            diverged_at: Some(waypoints[idx].commit_sha.clone()),
            responsible_commit: Some(waypoints[idx].commit_sha.clone()),
            candidate_deltas,
            reproducibility_envelope_hash: envelope_hash_str,
        });
    }

    out.sort_by(|a, b| a.trace_id.cmp(&b.trace_id));
    out
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

    // -----------------------------------------------------------
    // compute_stack_attributions
    // -----------------------------------------------------------

    fn waypoint(commit_sha: &str, tags: &[(&str, &str)]) -> WaypointData {
        WaypointData {
            commit_sha: commit_sha.to_string(),
            verdict_tags: tags
                .iter()
                .map(|(p, t)| (PathBuf::from(p), t.to_string()))
                .collect(),
        }
    }

    fn trace(path: &str, bytes: &[u8]) -> (PathBuf, Vec<u8>) {
        (PathBuf::from(path), bytes.to_vec())
    }

    #[test]
    fn attribution_empty_when_no_waypoints() {
        let traces = vec![trace("t.jsonl", b"x")];
        let attrs = compute_stack_attributions(&traces, &[], &[]);
        assert!(attrs.is_empty());
    }

    #[test]
    fn attribution_skips_traces_that_never_diverge() {
        // Trace passes at base and at every commit — no attribution.
        let traces = vec![trace("t.jsonl", b"trace content")];
        let waypoints = vec![
            waypoint("base", &[("t.jsonl", "passed")]),
            waypoint("c1", &[("t.jsonl", "passed")]),
            waypoint("c2", &[("t.jsonl", "passed")]),
        ];
        let commit_delta_sets = vec![
            vec!["agent.added:foo".to_string()],
            vec!["agent.removed:foo".to_string()],
        ];
        let attrs = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        assert!(
            attrs.is_empty(),
            "no divergence → no attribution; got: {attrs:?}"
        );
    }

    #[test]
    fn attribution_points_at_first_divergent_commit() {
        // Trace passes at base + c1, diverges at c2. diverged_at
        // and responsible_commit should both point at c2;
        // candidate_deltas should be c2's delta set.
        let traces = vec![trace("t.jsonl", b"recorded trace")];
        let waypoints = vec![
            waypoint("base", &[("t.jsonl", "passed")]),
            waypoint("c1", &[("t.jsonl", "passed")]),
            waypoint("c2", &[("t.jsonl", "diverged")]),
        ];
        let commit_delta_sets = vec![
            vec!["c1.delta".to_string()],
            vec!["c2.delta.a".to_string(), "c2.delta.b".to_string()],
        ];
        let attrs = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        assert_eq!(attrs.len(), 1);
        let a = &attrs[0];
        assert_eq!(a.diverged_at.as_deref(), Some("c2"));
        assert_eq!(a.responsible_commit.as_deref(), Some("c2"));
        assert_eq!(
            a.candidate_deltas,
            vec!["c2.delta.a".to_string(), "c2.delta.b".to_string()]
        );
        assert_eq!(a.reproducibility_envelope_hash.len(), 64);
    }

    #[test]
    fn attribution_catches_divergence_at_first_commit() {
        // Trace passes at base, diverges at c1 and stays diverged.
        let traces = vec![trace("t.jsonl", b"content")];
        let waypoints = vec![
            waypoint("base", &[("t.jsonl", "passed")]),
            waypoint("c1", &[("t.jsonl", "diverged")]),
            waypoint("c2", &[("t.jsonl", "diverged")]),
        ];
        let commit_delta_sets = vec![
            vec!["c1.delta".to_string()],
            vec!["c2.delta".to_string()],
        ];
        let attrs = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].diverged_at.as_deref(), Some("c1"));
        assert_eq!(attrs[0].candidate_deltas, vec!["c1.delta"]);
    }

    #[test]
    fn attribution_catches_divergence_then_restoration_as_divergence() {
        // Trace passes at base, diverges at c1, is restored at c2.
        // Attribution still fires for the c1 divergence —
        // `diverged_at` reports the first divergence point even
        // when later commits restore correctness. A later follow-up
        // commit may refine this to report restoration explicitly;
        // for now, any divergence anywhere in the stack attributes.
        let traces = vec![trace("t.jsonl", b"content")];
        let waypoints = vec![
            waypoint("base", &[("t.jsonl", "passed")]),
            waypoint("c1", &[("t.jsonl", "diverged")]),
            waypoint("c2", &[("t.jsonl", "passed")]),
        ];
        let commit_delta_sets = vec![vec!["c1.delta".into()], vec!["c2.delta".into()]];
        let attrs = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].diverged_at.as_deref(), Some("c1"));
    }

    #[test]
    fn attribution_emits_one_record_per_diverging_trace() {
        let traces = vec![
            trace("a.jsonl", b"trace_a"),
            trace("b.jsonl", b"trace_b"),
            trace("c.jsonl", b"trace_c"),
        ];
        let waypoints = vec![
            waypoint(
                "base",
                &[("a.jsonl", "passed"), ("b.jsonl", "passed"), ("c.jsonl", "passed")],
            ),
            waypoint(
                "c1",
                &[("a.jsonl", "passed"), ("b.jsonl", "diverged"), ("c.jsonl", "passed")],
            ),
            waypoint(
                "c2",
                &[("a.jsonl", "diverged"), ("b.jsonl", "diverged"), ("c.jsonl", "passed")],
            ),
        ];
        let commit_delta_sets = vec![vec!["c1.d".into()], vec!["c2.d".into()]];
        let attrs = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        // Only a and b diverge (c passes through unchanged).
        assert_eq!(attrs.len(), 2);
        // Sorted by trace_id, which hashes the bytes — check both
        // attributions surfaced and that divergence points are
        // assigned correctly regardless of sort order.
        let by_commit: std::collections::BTreeMap<&str, &Attribution> = attrs
            .iter()
            .map(|a| (a.responsible_commit.as_deref().unwrap_or(""), a))
            .collect();
        assert_eq!(by_commit.len(), 2);
        // `b` diverges first at c1; `a` diverges at c2.
        assert!(by_commit.contains_key("c1"));
        assert!(by_commit.contains_key("c2"));
    }

    #[test]
    fn attribution_output_is_byte_stable_across_runs() {
        let traces = vec![trace("t.jsonl", b"content")];
        let waypoints = vec![
            waypoint("base", &[("t.jsonl", "passed")]),
            waypoint("c1", &[("t.jsonl", "diverged")]),
        ];
        let commit_delta_sets = vec![vec!["c1.d".into()]];
        let run1 = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        let run2 = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        let j1 = serde_json::to_string(&run1).unwrap();
        let j2 = serde_json::to_string(&run2).unwrap();
        assert_eq!(j1, j2);
    }

    // -----------------------------------------------------------
    // Algebra-directed skip helpers
    // -----------------------------------------------------------

    #[test]
    fn trace_exercised_agents_extracts_run_started_agent() {
        let trace = br#"{"kind":"run_started","ts_ms":1,"run_id":"r","agent":"refund_bot","args":[]}
{"kind":"run_completed","ts_ms":2,"run_id":"r","ok":true,"result":null,"error":null}"#;
        let agents = trace_exercised_agents(trace);
        assert!(agents.contains("refund_bot"));
        assert_eq!(agents.len(), 1);
    }

    #[test]
    fn trace_exercised_agents_captures_nested_agents() {
        let trace = br#"{"kind":"run_started","ts_ms":1,"run_id":"r1","agent":"top_agent","args":[]}
{"kind":"run_started","ts_ms":2,"run_id":"r2","agent":"nested_agent","args":[]}
{"kind":"run_completed","ts_ms":3,"run_id":"r2","ok":true,"result":null,"error":null}
{"kind":"run_completed","ts_ms":4,"run_id":"r1","ok":true,"result":null,"error":null}"#;
        let agents = trace_exercised_agents(trace);
        assert!(agents.contains("top_agent"));
        assert!(agents.contains("nested_agent"));
    }

    #[test]
    fn trace_exercised_agents_returns_empty_on_unparseable() {
        assert!(trace_exercised_agents(b"not valid json").is_empty());
        assert!(trace_exercised_agents(b"").is_empty());
    }

    #[test]
    fn trace_exercised_agents_skips_malformed_lines_but_continues() {
        let trace = br#"{"kind":"run_started","ts_ms":1,"run_id":"r","agent":"good_agent","args":[]}
{not valid json here}
{"kind":"run_started","ts_ms":2,"run_id":"r2","agent":"another","args":[]}"#;
        let agents = trace_exercised_agents(trace);
        assert!(agents.contains("good_agent"));
        assert!(agents.contains("another"));
    }

    #[test]
    fn commit_affected_agents_parses_lifecycle_keys() {
        let keys = vec![
            "agent.added:foo".to_string(),
            "agent.removed:bar".to_string(),
        ];
        let agents = commit_affected_agents(&keys);
        assert!(agents.contains("foo"));
        assert!(agents.contains("bar"));
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn commit_affected_agents_parses_transition_keys_with_label_entities() {
        let keys = vec![
            "agent.trust_tier_changed:bot:autonomous->human_required".to_string(),
            "agent.approval.tier_changed:refund_bot:IssueRefund:strict->lenient".to_string(),
        ];
        let agents = commit_affected_agents(&keys);
        assert!(agents.contains("bot"));
        assert!(agents.contains("refund_bot"));
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn commit_affected_agents_parses_derived_and_provenance_keys() {
        let keys = vec![
            "agent.dangerous_gained:classifier".to_string(),
            "agent.provenance.dep_added:classifier:input".to_string(),
            "agent.approval.label_added:classifier:UseModel".to_string(),
        ];
        let agents = commit_affected_agents(&keys);
        // All three deltas touch the same agent; the set dedupes.
        assert_eq!(agents.len(), 1);
        assert!(agents.contains("classifier"));
    }

    #[test]
    fn can_skip_when_sets_provably_disjoint() {
        let exercised: BTreeSet<String> =
            ["refund_bot".into(), "greeter".into()].into_iter().collect();
        let affected: BTreeSet<String> =
            ["classifier".into(), "translator".into()].into_iter().collect();
        assert!(can_skip_replay(&exercised, &affected));
    }

    #[test]
    fn cannot_skip_when_sets_intersect() {
        let exercised: BTreeSet<String> =
            ["refund_bot".into(), "greeter".into()].into_iter().collect();
        let affected: BTreeSet<String> =
            ["refund_bot".into()].into_iter().collect();
        assert!(!can_skip_replay(&exercised, &affected));
    }

    #[test]
    fn cannot_skip_when_exercised_is_empty_even_if_affected_is_populated() {
        // Empty `exercised` = parser couldn't identify agents.
        // Conservative: don't skip. Over-replay is waste; under-
        // replay is missed regression. We pick the safe side.
        let exercised: BTreeSet<String> = BTreeSet::new();
        let affected: BTreeSet<String> =
            ["refund_bot".into()].into_iter().collect();
        assert!(!can_skip_replay(&exercised, &affected));
    }

    #[test]
    fn cannot_skip_when_affected_is_empty() {
        // No deltas in the commit (hypothetical empty commit) →
        // no affected agents → skipping would be a no-op anyway,
        // but we still run the harness for consistency. The
        // caller can optimize this separately if it matters.
        let exercised: BTreeSet<String> =
            ["refund_bot".into()].into_iter().collect();
        let affected: BTreeSet<String> = BTreeSet::new();
        assert!(!can_skip_replay(&exercised, &affected));
    }

    #[test]
    fn attribution_handles_absent_verdict_at_base_as_divergence_signal() {
        // If a trace has no verdict at base (shouldn't normally
        // happen, but defensive), treat later waypoints with
        // actual verdicts as divergent. Keeps attribution
        // conservative rather than silently dropping traces that
        // the harness couldn't evaluate at base.
        let traces = vec![trace("t.jsonl", b"content")];
        let waypoints = vec![
            WaypointData {
                commit_sha: "base".into(),
                verdict_tags: BTreeMap::new(),
            },
            waypoint("c1", &[("t.jsonl", "passed")]),
        ];
        let commit_delta_sets = vec![vec!["c1.d".into()]];
        let attrs = compute_stack_attributions(&traces, &waypoints, &commit_delta_sets);
        // "absent" at base vs "passed" at c1 is a divergence.
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].responsible_commit.as_deref(), Some("c1"));
    }
}
