#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayApprovalDecision {
    pub accepted: bool,
    pub decider: String,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayApprovalOutcome {
    pub approved: bool,
    pub decision: Option<ReplayApprovalDecision>,
}
