//! Test fixtures shared by the `observe_helpers_cmd` sibling
//! tests. Each leaf subcommand's tests need to seed lineage events
//! into a tempdir; consolidating the constructors here keeps every
//! sibling test module self-contained without copy-pasting the
//! `LineageEvent` field set.
//!
//! The module is `#[cfg(test)]` only — these helpers never compile
//! into the release binary.

use corvid_runtime::lineage::{LineageEvent, LineageKind, LineageStatus};
use std::fs;
use std::path::Path;

pub(crate) fn write_lineage(path: &Path, events: &[LineageEvent]) {
    let mut out = String::new();
    for e in events {
        out.push_str(&serde_json::to_string(e).unwrap());
        out.push('\n');
    }
    fs::write(path, out).unwrap();
}

pub(crate) fn ev(
    kind: LineageKind,
    name: &str,
    trace: &str,
    span: &str,
    status: LineageStatus,
    guarantee: &str,
    cost: f64,
) -> LineageEvent {
    LineageEvent {
        schema: corvid_runtime::lineage::LINEAGE_SCHEMA.to_string(),
        trace_id: trace.to_string(),
        span_id: span.to_string(),
        parent_span_id: String::new(),
        kind,
        name: name.to_string(),
        status,
        started_ms: 0,
        ended_ms: 100,
        tenant_id: "t1".to_string(),
        actor_id: "a1".to_string(),
        request_id: String::new(),
        replay_key: String::new(),
        idempotency_key: String::new(),
        guarantee_id: guarantee.to_string(),
        effect_ids: vec![],
        approval_id: String::new(),
        data_classes: vec![],
        cost_usd: cost,
        tokens_in: 0,
        tokens_out: 0,
        confidence: 0.0,
        latency_ms: 100,
        model_id: String::new(),
        model_fingerprint: String::new(),
        prompt_hash: String::new(),
        retrieval_index_hash: String::new(),
        input_fingerprint: String::new(),
        output_fingerprint: String::new(),
        redaction_policy_hash: String::new(),
    }
}
