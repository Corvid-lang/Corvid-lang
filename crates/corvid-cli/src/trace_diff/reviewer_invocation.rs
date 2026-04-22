//! Compile the in-repo Corvid reviewer, digest base/head `CorvidAbi`
//! descriptors into the shape the reviewer consumes, and invoke
//! `review_pr` through the interpreter.
//!
//! The reviewer is a `.cor` program (`reviewer.cor`) baked into the
//! binary via `include_str!`. That keeps the CLI self-contained:
//! there is no lookup path and no user configuration. The reviewer
//! runs in `@deterministic` mode, consumes a digested view of both
//! sides plus a `TraceImpact`, and returns the markdown receipt as
//! a `String`.
//!
//! The Rust-side types here mirror the reviewer's Corvid `type`
//! decls field-for-field; [`corvid_vm::json_to_value`] coerces
//! through that shape at the FFI boundary. Extra fields on
//! `CorvidAbi` that the reviewer doesn't care about are simply
//! absent from these shadow structs — `json_to_value` ignores them.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use corvid_abi::{
    AbiAgent, AbiApprovalContract, AbiApprovalLabel, AbiProvenanceContract, CorvidAbi,
};
use corvid_driver::{compile_to_ir, run_ir_with_runtime, Runtime};
use corvid_runtime::ProgrammaticApprover;
use corvid_vm::{json_to_value, Value};
use serde::Serialize;

use super::impact::TraceImpact;

/// The Corvid source of the reviewer agent. Baked into the binary
/// so the CLI is self-contained: no lookup path, no user
/// configuration, same reviewer behavior across every install of
/// the same `corvid` build. Users can still read + fork this file
/// in the repo (`crates/corvid-cli/src/trace_diff/reviewer.cor`).
pub(super) const REVIEWER_SOURCE: &str = include_str!("reviewer.cor");

/// The digested view the Corvid reviewer consumes. Mirrors the
/// `type` decls in [`reviewer.cor`] field-for-field; `json_to_value`
/// coerces through this shape at the FFI boundary. Extra fields on
/// `CorvidAbi` that the reviewer doesn't yet care about are simply
/// not included here — `json_to_value` ignores them.
#[derive(Serialize)]
struct Descriptor {
    agents: Vec<AgentSummary>,
}

#[derive(Serialize)]
struct AgentSummary {
    name: String,
    trust_tier: String,
    is_dangerous: bool,
    is_replayable: bool,
    approval: ApprovalContractSummary,
    provenance: ProvenanceSummary,
}

#[derive(Serialize)]
struct ApprovalContractSummary {
    required: bool,
    labels: Vec<ApprovalLabelSummary>,
}

/// Approval label surface visible to the reviewer. `required_tier`
/// and `reversibility` come from `AbiApprovalLabel` via
/// `Option<String>` — absent is normalised to the literal
/// `"unspecified"` so the Corvid side (which does not yet have an
/// Option surface for these fields) compares strings uniformly.
/// `cost_at_site` is deliberately omitted: Corvid does not yet
/// have a Float→String primitive, so numeric cost deltas stay
/// deferred to a follow-up slice rather than being pre-rendered in
/// Rust and collapsing the layering.
#[derive(Serialize)]
struct ApprovalLabelSummary {
    label: String,
    required_tier: String,
    reversibility: String,
}

#[derive(Serialize)]
struct ProvenanceSummary {
    returns_grounded: bool,
    grounded_param_deps: Vec<String>,
}

fn digest(abi: &CorvidAbi) -> Descriptor {
    Descriptor {
        agents: abi.agents.iter().map(digest_agent).collect(),
    }
}

fn digest_agent(agent: &AbiAgent) -> AgentSummary {
    AgentSummary {
        name: agent.name.clone(),
        trust_tier: agent
            .effects
            .trust_tier
            .clone()
            .unwrap_or_else(|| "unspecified".to_string()),
        is_dangerous: agent.attributes.dangerous,
        is_replayable: agent.attributes.replayable,
        approval: digest_approval(&agent.approval_contract),
        provenance: digest_provenance(&agent.provenance),
    }
}

fn digest_approval(contract: &AbiApprovalContract) -> ApprovalContractSummary {
    ApprovalContractSummary {
        required: contract.required,
        labels: contract.labels.iter().map(digest_approval_label).collect(),
    }
}

fn digest_approval_label(label: &AbiApprovalLabel) -> ApprovalLabelSummary {
    ApprovalLabelSummary {
        label: label.label.clone(),
        required_tier: label
            .required_tier
            .clone()
            .unwrap_or_else(|| "unspecified".to_string()),
        reversibility: label
            .reversibility
            .clone()
            .unwrap_or_else(|| "unspecified".to_string()),
    }
}

fn digest_provenance(contract: &AbiProvenanceContract) -> ProvenanceSummary {
    ProvenanceSummary {
        returns_grounded: contract.returns_grounded,
        grounded_param_deps: contract.grounded_param_deps.clone(),
    }
}

/// Compile the in-repo reviewer source, coerce descriptors +
/// impact to typed `Value`s, and run `review_pr` through the
/// interpreter.
pub(super) fn invoke_reviewer(
    base_abi: &CorvidAbi,
    head_abi: &CorvidAbi,
    impact: &TraceImpact,
) -> Result<String> {
    let reviewer_ir = compile_to_ir(REVIEWER_SOURCE)
        .map_err(|diags| anyhow!("reviewer source failed to compile: {} diagnostic(s)", diags.len()))?;

    let descriptor_type = reviewer_ir
        .types
        .iter()
        .find(|t| t.name == "Descriptor")
        .ok_or_else(|| anyhow!("reviewer source missing `Descriptor` type"))?;
    let impact_type = reviewer_ir
        .types
        .iter()
        .find(|t| t.name == "TraceImpact")
        .ok_or_else(|| anyhow!("reviewer source missing `TraceImpact` type"))?;
    let types_by_id = reviewer_ir.types.iter().map(|t| (t.id, t)).collect();

    let descriptor_expected = corvid_types::Type::Struct(descriptor_type.id);
    let impact_expected = corvid_types::Type::Struct(impact_type.id);

    let base_value = json_to_value(
        serde_json::to_value(digest(base_abi))?,
        &descriptor_expected,
        &types_by_id,
    )
    .map_err(|e| anyhow!("base descriptor → Value: {e:?}"))?;
    let head_value = json_to_value(
        serde_json::to_value(digest(head_abi))?,
        &descriptor_expected,
        &types_by_id,
    )
    .map_err(|e| anyhow!("head descriptor → Value: {e:?}"))?;
    let impact_value = json_to_value(
        serde_json::to_value(impact)?,
        &impact_expected,
        &types_by_id,
    )
    .map_err(|e| anyhow!("impact → Value: {e:?}"))?;

    // The reviewer is `@deterministic` and calls no LLMs, tools, or
    // approvers. A minimal runtime with a programmatic approver (any
    // policy — it will never be consulted) is enough to satisfy the
    // interpreter's required-approver invariant.
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_no()))
        .build();

    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for reviewer")?;

    let result = tokio_rt
        .block_on(run_ir_with_runtime(
            &reviewer_ir,
            Some("review_pr"),
            vec![base_value, head_value, impact_value],
            &runtime,
        ))
        .map_err(|e| anyhow!("reviewer `review_pr` failed: {e}"))?;

    match result {
        Value::String(s) => Ok(s.to_string()),
        other => Err(anyhow!(
            "reviewer `review_pr` returned non-String value: {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewer_source_compiles() {
        // The reviewer must be kept typecheck-clean at commit time.
        // This test fails at CI time if a refactor of corvid-types or
        // the Corvid surface breaks the reviewer without updating it.
        let ir = compile_to_ir(REVIEWER_SOURCE)
            .expect("embedded reviewer.cor compiles on every `cargo test`");
        assert!(
            ir.agents.iter().any(|a| a.name == "review_pr"),
            "reviewer IR must expose `review_pr` agent"
        );
        assert!(
            ir.types.iter().any(|t| t.name == "Descriptor"),
            "reviewer IR must expose `Descriptor` type"
        );
    }

    /// Synthesise a tiny `CorvidAbi` for testing via JSON round-trip.
    /// Cheaper than constructing `AbiAgent` field-by-field and stays
    /// resilient if future schema extensions add required fields:
    /// deserialization fills defaults where `#[serde(default)]` is set.
    fn synth_abi(agents: &[(&str, &str, bool, bool)]) -> CorvidAbi {
        let agents_json: Vec<serde_json::Value> = agents
            .iter()
            .map(|(name, trust, dangerous, replayable)| {
                serde_json::json!({
                    "name": name,
                    "symbol": name,
                    "source_span": { "start": 0, "end": 0 },
                    "params": [],
                    "return_type": { "kind": "scalar", "scalar": "Int" },
                    "effects": { "trust_tier": trust },
                    "attributes": {
                        "replayable": replayable,
                        "deterministic": false,
                        "dangerous": dangerous,
                        "pub_extern_c": false
                    },
                    "approval_contract": { "required": dangerous, "labels": [] },
                    "provenance": { "returns_grounded": false, "grounded_param_deps": [] }
                })
            })
            .collect();
        let json = serde_json::json!({
            "corvid_abi_version": corvid_abi::CORVID_ABI_VERSION,
            "compiler_version": "test",
            "source_path": "test.cor",
            "generated_at": "1970-01-01T00:00:00Z",
            "agents": agents_json,
            "prompts": [],
            "tools": [],
            "types": [],
            "approval_sites": []
        });
        corvid_abi::descriptor_from_json(&serde_json::to_string(&json).unwrap())
            .expect("synth_abi JSON deserializes to a CorvidAbi")
    }

    #[test]
    fn reviewer_reports_no_changes_when_both_sides_equal() {
        let base = synth_abi(&[("classify", "autonomous", false, true)]);
        let head = synth_abi(&[("classify", "autonomous", false, true)]);
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(receipt.contains("No algebraic changes detected"), "got: {receipt}");
    }

    #[test]
    fn reviewer_reports_added_agent() {
        let base = synth_abi(&[("classify", "autonomous", false, true)]);
        let head = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("summarize", "autonomous", false, true),
        ]);
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(receipt.contains("Added"), "got: {receipt}");
        assert!(receipt.contains("summarize"), "got: {receipt}");
    }

    #[test]
    fn reviewer_reports_removed_agent() {
        let base = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("summarize", "autonomous", false, true),
        ]);
        let head = synth_abi(&[("classify", "autonomous", false, true)]);
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(receipt.contains("Removed"), "got: {receipt}");
        assert!(receipt.contains("summarize"), "got: {receipt}");
    }

    #[test]
    fn reviewer_flags_dangerous_transition() {
        let base = synth_abi(&[("refund_bot", "human_required", false, false)]);
        let head = synth_abi(&[("refund_bot", "human_required", true, false)]);
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(receipt.contains("became `@dangerous`"), "got: {receipt}");
        assert!(receipt.contains("refund_bot"), "got: {receipt}");
    }

    #[test]
    fn reviewer_flags_trust_tier_change() {
        let base = synth_abi(&[("refund_bot", "human_required", false, false)]);
        let head = synth_abi(&[("refund_bot", "autonomous", false, false)]);
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(receipt.contains("trust changed"), "got: {receipt}");
        assert!(receipt.contains("human_required"), "got: {receipt}");
        assert!(receipt.contains("autonomous"), "got: {receipt}");
    }

    #[test]
    fn reviewer_is_deterministic_across_calls() {
        let base = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("refund_bot", "human_required", true, false),
        ]);
        let head = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("summarize", "autonomous", false, false),
        ]);
        let first = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        let second = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert_eq!(first, second, "@deterministic reviewer must produce byte-identical receipts");
    }

    // -------------------- trace-impact rendering --------------------

    /// Empty impact (no `--traces` flag) must render zero content
    /// for its section — slice-1 receipts continue to look exactly
    /// like slice-1 receipts when no counterfactual corpus is
    /// supplied.
    #[test]
    fn empty_impact_renders_no_section() {
        let base = synth_abi(&[("classify", "autonomous", false, true)]);
        let head = synth_abi(&[("classify", "autonomous", false, true)]);
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            !receipt.contains("Counterfactual Replay Impact"),
            "empty impact must not render a section; got: {receipt}"
        );
    }

    fn synth_impact(any_newly_diverged: bool, newly_paths: Vec<&str>) -> TraceImpact {
        TraceImpact {
            has_traces: true,
            any_newly_diverged,
            summary_line: "Replayed 10 trace(s) against base and head: ...".into(),
            impact_percentage: "20.0%".into(),
            newly_diverged_paths: newly_paths.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn impact_section_renders_when_traces_present() {
        let base = synth_abi(&[("classify", "autonomous", false, true)]);
        let head = synth_abi(&[("classify", "autonomous", false, true)]);
        let impact = synth_impact(true, vec!["run-a.jsonl", "run-b.jsonl"]);
        let receipt = invoke_reviewer(&base, &head, &impact).unwrap();
        assert!(receipt.contains("Counterfactual Replay Impact"), "got: {receipt}");
        assert!(receipt.contains("Newly divergent under head"), "got: {receipt}");
        assert!(receipt.contains("run-a.jsonl"), "got: {receipt}");
        assert!(receipt.contains("run-b.jsonl"), "got: {receipt}");
        assert!(receipt.contains("20.0%"), "got: {receipt}");
    }

    #[test]
    fn impact_section_renders_clean_when_no_newly_diverged() {
        let base = synth_abi(&[("classify", "autonomous", false, true)]);
        let head = synth_abi(&[("classify", "autonomous", false, true)]);
        let impact = synth_impact(false, vec![]);
        let receipt = invoke_reviewer(&base, &head, &impact).unwrap();
        assert!(receipt.contains("Counterfactual Replay Impact"), "got: {receipt}");
        assert!(
            receipt.contains("No traces newly diverge under this PR"),
            "got: {receipt}"
        );
        assert!(
            !receipt.contains("Newly divergent under head"),
            "clean impact must not list a (would-be empty) path section; got: {receipt}"
        );
    }

    /// Sibling of `synth_abi` for tests that need to exercise the
    /// approval-contract + provenance fields. `approval_labels` is a
    /// list of `(label, required_tier, reversibility)` tuples; empty
    /// tiers/reversibilities are rendered as the `"unspecified"`
    /// normalised form. `grounded_deps` goes verbatim into
    /// `grounded_param_deps`; `returns_grounded` is the explicit
    /// flag.
    fn synth_abi_with_contracts(
        name: &str,
        trust: &str,
        dangerous: bool,
        replayable: bool,
        approval_labels: &[(&str, &str, &str)],
        returns_grounded: bool,
        grounded_deps: &[&str],
    ) -> CorvidAbi {
        let labels_json: Vec<serde_json::Value> = approval_labels
            .iter()
            .map(|(label, tier, rev)| {
                let mut v = serde_json::json!({
                    "label": label,
                    "args": [],
                });
                if !tier.is_empty() {
                    v["required_tier"] = serde_json::Value::String(tier.to_string());
                }
                if !rev.is_empty() {
                    v["reversibility"] = serde_json::Value::String(rev.to_string());
                }
                v
            })
            .collect();
        let grounded_deps_json: Vec<serde_json::Value> = grounded_deps
            .iter()
            .map(|d| serde_json::Value::String(d.to_string()))
            .collect();
        let agent_json = serde_json::json!({
            "name": name,
            "symbol": name,
            "source_span": { "start": 0, "end": 0 },
            "params": [],
            "return_type": { "kind": "scalar", "scalar": "Int" },
            "effects": { "trust_tier": trust },
            "attributes": {
                "replayable": replayable,
                "deterministic": false,
                "dangerous": dangerous,
                "pub_extern_c": false
            },
            "approval_contract": {
                "required": dangerous,
                "labels": labels_json,
            },
            "provenance": {
                "returns_grounded": returns_grounded,
                "grounded_param_deps": grounded_deps_json,
            }
        });
        let json = serde_json::json!({
            "corvid_abi_version": corvid_abi::CORVID_ABI_VERSION,
            "compiler_version": "test",
            "source_path": "test.cor",
            "generated_at": "1970-01-01T00:00:00Z",
            "agents": [agent_json],
            "prompts": [],
            "tools": [],
            "types": [],
            "approval_sites": []
        });
        corvid_abi::descriptor_from_json(&serde_json::to_string(&json).unwrap())
            .expect("synth_abi_with_contracts JSON deserializes to a CorvidAbi")
    }

    #[test]
    fn reviewer_reports_added_approval_label() {
        let base = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "human_required", "reversible")],
            false,
            &[],
        );
        let head = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[
                ("IssueRefund", "human_required", "reversible"),
                ("WireTransfer", "human_required", "irreversible"),
            ],
            false,
            &[],
        );
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            receipt.contains("approve site `WireTransfer` added"),
            "got: {receipt}"
        );
    }

    #[test]
    fn reviewer_reports_removed_approval_label() {
        let base = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[
                ("IssueRefund", "human_required", "reversible"),
                ("WireTransfer", "human_required", "irreversible"),
            ],
            false,
            &[],
        );
        let head = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "human_required", "reversible")],
            false,
            &[],
        );
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            receipt.contains("approve site `WireTransfer` removed"),
            "got: {receipt}"
        );
    }

    #[test]
    fn reviewer_flags_weakened_required_tier_on_approval_label() {
        let base = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "human_required", "reversible")],
            false,
            &[],
        );
        let head = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "autonomous", "reversible")],
            false,
            &[],
        );
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            receipt
                .contains("approve site `IssueRefund` required-tier: `human_required` -> `autonomous`"),
            "got: {receipt}"
        );
    }

    #[test]
    fn reviewer_flags_reversibility_regression_on_approval_label() {
        let base = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "human_required", "reversible")],
            false,
            &[],
        );
        let head = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "human_required", "irreversible")],
            false,
            &[],
        );
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            receipt.contains("approve site `IssueRefund` became irreversible"),
            "got: {receipt}"
        );
    }

    #[test]
    fn reviewer_flags_gained_grounded_return() {
        let base = synth_abi_with_contracts(
            "answer_question",
            "human_required",
            false,
            false,
            &[],
            false,
            &[],
        );
        let head = synth_abi_with_contracts(
            "answer_question",
            "human_required",
            false,
            false,
            &[],
            true,
            &["source_docs"],
        );
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            receipt.contains("return value gained `Grounded<T>` provenance"),
            "got: {receipt}"
        );
        assert!(
            receipt.contains("grounded dependency on `source_docs` added"),
            "got: {receipt}"
        );
    }

    #[test]
    fn reviewer_flags_lost_grounded_return() {
        let base = synth_abi_with_contracts(
            "answer_question",
            "human_required",
            false,
            false,
            &[],
            true,
            &["source_docs"],
        );
        let head = synth_abi_with_contracts(
            "answer_question",
            "human_required",
            false,
            false,
            &[],
            false,
            &[],
        );
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            receipt.contains("return value lost `Grounded<T>` provenance"),
            "got: {receipt}"
        );
        assert!(
            receipt.contains("grounded dependency on `source_docs` removed"),
            "got: {receipt}"
        );
    }

    #[test]
    fn reviewer_reports_no_changes_when_contracts_are_identical() {
        let base = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "human_required", "reversible")],
            true,
            &["ticket"],
        );
        let head = synth_abi_with_contracts(
            "refund_bot",
            "human_required",
            true,
            false,
            &[("IssueRefund", "human_required", "reversible")],
            true,
            &["ticket"],
        );
        let receipt = invoke_reviewer(&base, &head, &TraceImpact::empty()).unwrap();
        assert!(
            receipt.contains("No algebraic changes detected"),
            "got: {receipt}"
        );
    }
}
