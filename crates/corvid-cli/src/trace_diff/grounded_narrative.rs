//! Host-side minting for trace-diff receipt narratives.
//!
//! `reviewer.cor` now asks for `Grounded<ReceiptNarrative>` so the
//! deterministic reviewer cannot accidentally consume uncited prose as
//! plain data. The LLM prompt still returns a plain `ReceiptNarrative`;
//! Rust validates every cited delta key against the compiler-derived
//! diff summary before this module wraps the value in runtime
//! provenance.

use corvid_runtime::provenance::{ProvenanceChain, ProvenanceEntry, ProvenanceKind};
use corvid_vm::{GroundedValue, Value};

use super::narrative::ReceiptNarrative;

const TRACE_DIFF_DELTA_SOURCE_PREFIX: &str = "trace-diff.delta:";

/// Convert a validated receipt narrative VM value into a grounded
/// value. Each citation becomes a provenance source pointing at the
/// canonical delta key that Rust already validated. The empty
/// narrative sentinel carries no prose claims, so it intentionally
/// mints an empty chain rather than inventing a fake citation source.
pub(super) fn ground_receipt_narrative(value: Value, narrative: &ReceiptNarrative) -> Value {
    Value::Grounded(GroundedValue::new(
        value,
        provenance_from_narrative(narrative),
    ))
}

fn provenance_from_narrative(narrative: &ReceiptNarrative) -> ProvenanceChain {
    let mut chain = ProvenanceChain::new();
    for citation in &narrative.citations {
        chain.entries.push(ProvenanceEntry {
            kind: ProvenanceKind::Retrieval,
            name: format!(
                "{}{}",
                TRACE_DIFF_DELTA_SOURCE_PREFIX, citation.delta_key
            ),
            // Receipt rendering is byte-deterministic; provenance
            // timestamps for compiler-derived delta keys must be too.
            timestamp_ms: 0,
        });
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::super::narrative::DeltaCitation;
    use super::*;

    #[test]
    fn grounded_narrative_uses_validated_delta_keys_as_sources() {
        let narrative = ReceiptNarrative {
            body: "Refund bot gained an approval gate.".to_string(),
            citations: vec![
                DeltaCitation {
                    delta_key: "agent.approval.label_added:refund_bot:IssueRefund".to_string(),
                },
                DeltaCitation {
                    delta_key: "agent.provenance.grounded_gained:explain".to_string(),
                },
            ],
        };

        let grounded = ground_receipt_narrative(Value::String("body".into()), &narrative);
        let Value::Grounded(grounded) = grounded else {
            panic!("narrative must be wrapped in Grounded<T>");
        };

        assert_eq!(grounded.provenance.entries.len(), 2);
        assert_eq!(
            grounded.provenance.entries[0].name,
            "trace-diff.delta:agent.approval.label_added:refund_bot:IssueRefund"
        );
        assert_eq!(grounded.provenance.entries[0].kind, ProvenanceKind::Retrieval);
        assert_eq!(grounded.provenance.entries[0].timestamp_ms, 0);
    }

    #[test]
    fn empty_narrative_mints_empty_provenance_chain() {
        let grounded =
            ground_receipt_narrative(Value::String("".into()), &ReceiptNarrative::empty());
        let Value::Grounded(grounded) = grounded else {
            panic!("narrative must be wrapped in Grounded<T>");
        };

        assert!(grounded.provenance.entries.is_empty());
    }
}
