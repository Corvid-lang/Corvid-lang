use super::differential::RunCompletionDivergence;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplayMutationReport {
    pub divergences: Vec<MutationDivergence>,
    pub run_completion_divergence: Option<RunCompletionDivergence>,
}

impl ReplayMutationReport {
    pub fn is_empty(&self) -> bool {
        self.divergences.is_empty() && self.run_completion_divergence.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationDivergence {
    pub step: usize,
    pub kind: String,
    pub recorded: serde_json::Value,
    pub got: serde_json::Value,
}
