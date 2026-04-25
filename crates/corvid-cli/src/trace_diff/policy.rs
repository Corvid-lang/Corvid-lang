//! User-replaceable trace-diff policy execution.
//!
//! The important boundary is structural: Rust digests receipt deltas
//! into typed policy facts, then Corvid code decides the verdict.
//! Policy programs do not parse key strings to discover safety
//! meaning; they receive `PolicyDelta` records with category,
//! operation, direction, and safety class already separated.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use corvid_driver::{compile_to_ir, run_ir_with_runtime, Runtime};
use corvid_runtime::ProgrammaticApprover;
use corvid_vm::{json_to_value, value_to_json};
use serde::Serialize;

use super::impact::TraceImpact;
use super::narrative::DeltaRecord;
use super::receipt::{
    is_ownership_loosening, is_trust_lowering, regression_flag_for, Receipt, Verdict,
};

const POLICY_PRELUDE: &str = include_str!("policy_prelude.cor");
const DEFAULT_POLICY: &str = include_str!("default_policy.cor");

#[derive(Debug, Clone, Serialize)]
struct PolicyReceipt {
    schema_version: i64,
    base_sha: String,
    head_sha: String,
    source_path: String,
    deltas: Vec<PolicyDelta>,
    impact: PolicyImpact,
    narrative_rejected: bool,
}

#[derive(Debug, Clone, Serialize)]
struct PolicyDelta {
    key: String,
    summary: String,
    category: String,
    operation: String,
    subject: String,
    direction: String,
    safety_class: String,
    from_value: String,
    to_value: String,
}

#[derive(Debug, Clone, Serialize)]
struct PolicyImpact {
    has_traces: bool,
    any_newly_diverged: bool,
    summary_line: String,
    impact_percentage: String,
    newly_diverged_paths: Vec<String>,
}

pub(super) fn apply_policy(receipt: &Receipt, policy_path: Option<&Path>) -> Result<Verdict> {
    let policy_body = match policy_path {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading trace-diff policy `{}`", path.display()))?,
        None => DEFAULT_POLICY.to_string(),
    };
    let source = format!("{POLICY_PRELUDE}\n\n{policy_body}");
    let policy_ir = compile_to_ir(&source).map_err(|diags| {
        anyhow!(
            "trace-diff policy failed to compile: {} diagnostic(s)",
            diags.len()
        )
    })?;
    let receipt_type = policy_ir
        .types
        .iter()
        .find(|t| t.name == "PolicyReceipt")
        .ok_or_else(|| anyhow!("trace-diff policy prelude missing `PolicyReceipt` type"))?;
    let types_by_id = policy_ir
        .types
        .iter()
        .map(|t| (t.id, t))
        .collect::<HashMap<_, _>>();
    let receipt_expected = corvid_types::Type::Struct(receipt_type.id);
    let receipt_value = json_to_value(
        serde_json::to_value(policy_receipt(receipt))?,
        &receipt_expected,
        &types_by_id,
    )
    .map_err(|e| anyhow!("trace-diff policy receipt -> Value: {e:?}"))?;

    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_no()))
        .build();
    let result = std::thread::Builder::new()
        .name("corvid-trace-diff-policy".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let tokio_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("build tokio runtime for trace-diff policy")?;
            tokio_rt
                .block_on(run_ir_with_runtime(
                    &policy_ir,
                    Some("apply_policy"),
                    vec![receipt_value],
                    &runtime,
                ))
                .map_err(|e| anyhow!("trace-diff policy `apply_policy` failed: {e}"))
        })
        .context("spawn trace-diff policy thread")?
        .join()
        .map_err(|_| anyhow!("trace-diff policy thread panicked"))??;

    serde_json::from_value(value_to_json(&result))
        .map_err(|e| anyhow!("trace-diff policy returned non-Verdict value: {e}"))
}

fn policy_receipt(receipt: &Receipt) -> PolicyReceipt {
    PolicyReceipt {
        schema_version: receipt.schema_version as i64,
        base_sha: receipt.base_sha.clone(),
        head_sha: receipt.head_sha.clone(),
        source_path: receipt.source_path.clone(),
        deltas: receipt.deltas.iter().map(policy_delta).collect(),
        impact: policy_impact(&receipt.impact),
        narrative_rejected: receipt.narrative_rejected,
    }
}

fn policy_impact(impact: &TraceImpact) -> PolicyImpact {
    PolicyImpact {
        has_traces: impact.has_traces,
        any_newly_diverged: impact.any_newly_diverged,
        summary_line: impact.summary_line.clone(),
        impact_percentage: impact.impact_percentage.clone(),
        newly_diverged_paths: impact.newly_diverged_paths.clone(),
    }
}

fn policy_delta(delta: &DeltaRecord) -> PolicyDelta {
    let parsed = parse_delta_key(&delta.key);
    let safety_class = safety_class(delta, &parsed);
    PolicyDelta {
        key: delta.key.clone(),
        summary: delta.summary.clone(),
        category: parsed.category,
        operation: parsed.operation,
        subject: parsed.subject,
        direction: parsed.direction,
        safety_class,
        from_value: parsed.from_value,
        to_value: parsed.to_value,
    }
}

#[derive(Debug, Clone)]
struct ParsedDelta {
    category: String,
    operation: String,
    subject: String,
    direction: String,
    from_value: String,
    to_value: String,
}

fn parse_delta_key(key: &str) -> ParsedDelta {
    let mut parts = key.split(':');
    let head = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();
    let mut head_parts = head.split('.');
    let category = head_parts.next().unwrap_or("").to_string();
    let operation = head_parts.collect::<Vec<_>>().join(".");
    let subject = args.first().copied().unwrap_or("").to_string();
    let transition = args
        .iter()
        .rev()
        .copied()
        .find(|part| part.contains("->"))
        .unwrap_or("");
    let (from_value, to_value) = transition
        .split_once("->")
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .unwrap_or_else(|| ("".to_string(), "".to_string()));
    let direction = classify_direction(&operation, transition);
    ParsedDelta {
        category,
        operation,
        subject,
        direction,
        from_value,
        to_value,
    }
}

fn classify_direction(operation: &str, transition: &str) -> String {
    if operation.ends_with("gained") || operation.ends_with("added") {
        return "added".to_string();
    }
    if operation.ends_with("lost") || operation.ends_with("removed") {
        return "removed".to_string();
    }
    if operation == "trust_tier_changed" || operation == "approval.tier_changed" {
        return if is_trust_lowering(transition) {
            "lowered".to_string()
        } else {
            "raised_or_lateral".to_string()
        };
    }
    if operation == "approval.reversibility_changed" {
        return if transition.ends_with("->irreversible") {
            "became_irreversible".to_string()
        } else {
            "became_reversible".to_string()
        };
    }
    if operation == "extern.ownership_changed" {
        return if is_ownership_loosening(transition) {
            "loosened".to_string()
        } else {
            "tightened_or_lateral".to_string()
        };
    }
    if transition.is_empty() {
        "informational".to_string()
    } else {
        "changed".to_string()
    }
}

fn safety_class(delta: &DeltaRecord, parsed: &ParsedDelta) -> String {
    if regression_flag_for(delta).is_some() {
        "regression".to_string()
    } else if matches!(
        parsed.direction.as_str(),
        "raised_or_lateral" | "became_reversible" | "tightened_or_lateral"
    ) || parsed.operation.ends_with("gained")
        || parsed.operation.ends_with("added")
    {
        "improvement".to_string()
    } else {
        "informational".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_diff::impact::TraceImpact;
    use crate::trace_diff::receipt::{apply_default_policy, RECEIPT_SCHEMA_VERSION};

    fn delta(key: &str, summary: &str) -> DeltaRecord {
        DeltaRecord {
            key: key.to_string(),
            summary: summary.to_string(),
        }
    }

    fn receipt(deltas: Vec<DeltaRecord>, impact: TraceImpact) -> Receipt {
        Receipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            base_sha: "base".into(),
            head_sha: "head".into(),
            source_path: "agent.cor".into(),
            deltas,
            impact,
            narrative: super::super::narrative::ReceiptNarrative::empty(),
            narrative_rejected: false,
        }
    }

    #[test]
    fn default_corvid_policy_matches_rust_policy_for_regression() {
        let r = receipt(
            vec![delta(
                "agent.extern.ownership_changed:greet:#0:@owned->@borrowed",
                "argument #0 ownership loosened",
            )],
            TraceImpact::empty(),
        );
        let rust = apply_default_policy(&r);
        let corvid = apply_policy(&r, None).unwrap();
        assert_eq!(corvid.ok, rust.ok);
        assert_eq!(corvid.flags, rust.flags);
    }

    #[test]
    fn default_corvid_policy_matches_rust_policy_for_clean_receipt() {
        let r = receipt(
            vec![delta("agent.added:summarize", "new agent `summarize`")],
            TraceImpact::empty(),
        );
        let rust = apply_default_policy(&r);
        let corvid = apply_policy(&r, None).unwrap();
        assert_eq!(corvid.ok, rust.ok);
        assert_eq!(corvid.flags, rust.flags);
    }

    #[test]
    fn policy_delta_exposes_structured_safety_fact() {
        let d = policy_delta(&delta(
            "agent.trust_tier_changed:refund_bot:human_required->autonomous",
            "trust lowered",
        ));
        assert_eq!(d.category, "agent");
        assert_eq!(d.operation, "trust_tier_changed");
        assert_eq!(d.subject, "refund_bot");
        assert_eq!(d.direction, "lowered");
        assert_eq!(d.safety_class, "regression");
        assert_eq!(d.from_value, "human_required");
        assert_eq!(d.to_value, "autonomous");
    }
}
