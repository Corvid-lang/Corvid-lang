//! Shared provenance-chain types.
//!
//! These live in `corvid-runtime` so the interpreter, native FFI
//! boundary, and future host-mint/query slices all speak the same
//! provenance shape.

use serde::{Deserialize, Serialize};

/// A value paired with an explicit provenance chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundedValue<T> {
    pub value: T,
    pub provenance: ProvenanceChain,
}

/// The provenance chain: every retrieval source, prompt transformation,
/// and agent handoff that a value passed through.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceChain {
    pub entries: Vec<ProvenanceEntry>,
}

/// One step in the provenance chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub kind: ProvenanceKind,
    pub name: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvenanceKind {
    /// Data retrieved from an external source (tool with `data: grounded`).
    Retrieval,
    /// Data transformed by an LLM prompt.
    PromptTransform,
    /// Data passed through an agent call.
    AgentHandoff,
    /// Provenance deliberately severed by `.unwrap(reason: ...)`.
    Severed { reason: String },
}

impl ProvenanceKind {
    pub fn label(&self) -> &str {
        match self {
            ProvenanceKind::Retrieval => "retrieval",
            ProvenanceKind::PromptTransform => "prompt",
            ProvenanceKind::AgentHandoff => "agent",
            ProvenanceKind::Severed { .. } => "severed",
        }
    }
}

impl ProvenanceChain {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn with_retrieval(tool_name: &str, timestamp_ms: u64) -> Self {
        Self {
            entries: vec![ProvenanceEntry {
                kind: ProvenanceKind::Retrieval,
                name: tool_name.to_string(),
                timestamp_ms,
            }],
        }
    }

    pub fn add_prompt_transform(&mut self, prompt_name: &str, timestamp_ms: u64) {
        self.entries.push(ProvenanceEntry {
            kind: ProvenanceKind::PromptTransform,
            name: prompt_name.to_string(),
            timestamp_ms,
        });
    }

    pub fn add_agent_handoff(&mut self, agent_name: &str, timestamp_ms: u64) {
        self.entries.push(ProvenanceEntry {
            kind: ProvenanceKind::AgentHandoff,
            name: agent_name.to_string(),
            timestamp_ms,
        });
    }

    pub fn merge(&mut self, other: &ProvenanceChain) {
        for entry in &other.entries {
            if !self
                .entries
                .iter()
                .any(|candidate| candidate.name == entry.name && candidate.kind == entry.kind)
            {
                self.entries.push(entry.clone());
            }
        }
    }

    pub fn has_retrieval(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.kind == ProvenanceKind::Retrieval)
    }

    pub fn has_source(&self, name: &str) -> bool {
        self.entries.iter().any(|entry| entry.name == name)
    }
}

impl<T> GroundedValue<T> {
    pub fn new(value: T, provenance: ProvenanceChain) -> Self {
        Self { value, provenance }
    }

    pub fn has_retrieval(&self) -> bool {
        self.provenance.has_retrieval()
    }

    pub fn map<U, F>(self, map: F) -> GroundedValue<U>
    where
        F: FnOnce(T) -> U,
    {
        GroundedValue {
            value: map(self.value),
            provenance: self.provenance,
        }
    }
}

