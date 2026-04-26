use corvid_trace_schema::TraceEvent;

pub(crate) fn is_initial_metadata(event: &TraceEvent) -> bool {
    matches!(event, TraceEvent::SchemaHeader { .. })
        || matches!(
            event,
            TraceEvent::SeedRead { purpose, .. } if purpose == "rollout_default_seed"
        )
}

pub(crate) fn is_dispatch_metadata(event: &TraceEvent) -> bool {
    matches!(
        event,
        TraceEvent::ModelSelected { .. }
            | TraceEvent::HostEvent { .. }
            | TraceEvent::ProgressiveEscalation { .. }
            | TraceEvent::ProgressiveExhausted { .. }
            | TraceEvent::StreamUpgrade { .. }
            | TraceEvent::PromptCache { .. }
            | TraceEvent::ApprovalTokenIssued { .. }
            | TraceEvent::AbVariantChosen { .. }
            | TraceEvent::EnsembleVote { .. }
            | TraceEvent::AdversarialPipelineCompleted { .. }
            | TraceEvent::AdversarialContradiction { .. }
            | TraceEvent::ProvenanceEdge { .. }
    )
}
