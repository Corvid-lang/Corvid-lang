#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalContractRecord {
    pub id: String,
    pub version: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub tenant_id: String,
    pub required_role: String,
    pub max_cost_usd: f64,
    pub data_class: String,
    pub irreversible: bool,
    pub expires_ms: u64,
    pub replay_key: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalCreate {
    pub id: String,
    pub tenant_id: String,
    pub requester_actor_id: String,
    pub contract: ApprovalContractRecord,
    pub risk_level: String,
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalQueueRecord {
    pub id: String,
    pub tenant_id: String,
    pub requester_actor_id: String,
    pub approver_actor_id: Option<String>,
    pub delegated_to_actor_id: Option<String>,
    pub contract_id: String,
    pub contract_version: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub status: String,
    pub required_role: String,
    pub risk_level: String,
    pub data_class: String,
    pub irreversible: bool,
    pub max_cost_usd: f64,
    pub expires_ms: u64,
    pub trace_id: String,
    pub replay_key: String,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalQueueAuditEvent {
    pub id: String,
    pub approval_id: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub event_kind: String,
    pub status_before: String,
    pub status_after: String,
    pub reason: Option<String>,
    pub trace_id: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalAuditCoverage {
    pub approval_id: String,
    pub tenant_id: String,
    pub trace_id: String,
    pub current_status: String,
    pub event_count: usize,
    pub has_create: bool,
    pub has_terminal_transition: bool,
    pub complete: bool,
}
