use crate::approval_queue::{ApprovalQueueAuditEvent, ApprovalQueueRecord};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalUiPayload {
    pub schema_version: String,
    pub id: String,
    pub tenant_id: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub risk_level: String,
    pub action: String,
    pub target: ApprovalUiTarget,
    pub requester_actor_id: String,
    pub approver_actor_id: Option<String>,
    pub delegated_to_actor_id: Option<String>,
    pub required_role: String,
    pub data_class: String,
    pub irreversible: bool,
    pub max_cost_usd: f64,
    pub expires_ms: u64,
    pub trace_id: String,
    pub replay_key: String,
    pub allowed_transitions: Vec<String>,
    pub audit: Vec<ApprovalUiAuditEvent>,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalUiTarget {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalUiAuditEvent {
    pub event_kind: String,
    pub actor_id: String,
    pub status_before: String,
    pub status_after: String,
    pub trace_id: String,
    pub created_ms: u64,
}

pub fn approval_ui_payload(
    approval: &ApprovalQueueRecord,
    audit: &[ApprovalQueueAuditEvent],
    viewer_actor_id: &str,
    viewer_role: &str,
) -> ApprovalUiPayload {
    ApprovalUiPayload {
        schema_version: "corvid.approval.ui.v1".to_string(),
        id: approval.id.clone(),
        tenant_id: approval.tenant_id.clone(),
        title: human_title(&approval.action),
        summary: format!(
            "{} on {} `{}`",
            human_title(&approval.action),
            approval.target_kind,
            approval.target_id
        ),
        status: approval.status.clone(),
        risk_level: approval.risk_level.clone(),
        action: approval.action.clone(),
        target: ApprovalUiTarget {
            kind: approval.target_kind.clone(),
            id: approval.target_id.clone(),
        },
        requester_actor_id: approval.requester_actor_id.clone(),
        approver_actor_id: approval.approver_actor_id.clone(),
        delegated_to_actor_id: approval.delegated_to_actor_id.clone(),
        required_role: approval.required_role.clone(),
        data_class: approval.data_class.clone(),
        irreversible: approval.irreversible,
        max_cost_usd: approval.max_cost_usd,
        expires_ms: approval.expires_ms,
        trace_id: approval.trace_id.clone(),
        replay_key: approval.replay_key.clone(),
        allowed_transitions: allowed_transitions(approval, viewer_actor_id, viewer_role),
        audit: audit.iter().map(ui_audit_event).collect(),
        redacted: true,
    }
}

fn allowed_transitions(
    approval: &ApprovalQueueRecord,
    viewer_actor_id: &str,
    viewer_role: &str,
) -> Vec<String> {
    if approval.status != "pending" {
        return Vec::new();
    }
    let delegated_match = approval
        .delegated_to_actor_id
        .as_deref()
        .map(|delegated| delegated == viewer_actor_id)
        .unwrap_or(true);
    if viewer_role != approval.required_role || !delegated_match {
        return vec!["comment".to_string()];
    }
    vec![
        "approve".to_string(),
        "deny".to_string(),
        "comment".to_string(),
        "delegate".to_string(),
    ]
}

fn ui_audit_event(event: &ApprovalQueueAuditEvent) -> ApprovalUiAuditEvent {
    ApprovalUiAuditEvent {
        event_kind: event.event_kind.clone(),
        actor_id: event.actor_id.clone(),
        status_before: event.status_before.clone(),
        status_after: event.status_after.clone(),
        trace_id: event.trace_id.clone(),
        created_ms: event.created_ms,
    }
}

fn human_title(action: &str) -> String {
    let mut out = String::new();
    for (index, part) in action
        .replace('-', "_")
        .split('_')
        .filter(|part| !part.is_empty())
        .enumerate()
    {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        "Approval".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_queue::{
        ApprovalContractRecord, ApprovalCreate, ApprovalQueueRuntime,
    };
    use crate::tracing::now_ms;

    fn contract(expires_ms: u64) -> ApprovalContractRecord {
        ApprovalContractRecord {
            id: "contract-1".to_string(),
            version: "v1".to_string(),
            action: "send_executive_follow_up".to_string(),
            target_kind: "email_thread".to_string(),
            target_id: "thread-1".to_string(),
            tenant_id: "org-1".to_string(),
            required_role: "Reviewer".to_string(),
            max_cost_usd: 0.25,
            data_class: "private".to_string(),
            irreversible: true,
            expires_ms,
            replay_key: "replay-approval-1".to_string(),
            created_ms: 0,
        }
    }

    #[test]
    fn approval_ui_payload_is_stable_and_frontend_agnostic() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let approval = queue
            .create(ApprovalCreate {
                id: "approval-1".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(now_ms().saturating_add(60_000)),
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-1".to_string(),
            })
            .unwrap();
        queue
            .comment(&approval.id, "org-1", "reviewer-1", "reviewing")
            .unwrap();
        let audit = queue.audit_events(&approval.id).unwrap();
        let payload = approval_ui_payload(&approval, &audit, "reviewer-1", "Reviewer");

        assert_eq!(payload.schema_version, "corvid.approval.ui.v1");
        assert_eq!(payload.title, "Send Executive Follow Up");
        assert_eq!(payload.target.kind, "email_thread");
        assert_eq!(payload.allowed_transitions, vec![
            "approve".to_string(),
            "deny".to_string(),
            "comment".to_string(),
            "delegate".to_string(),
        ]);
        assert_eq!(payload.audit.len(), 2);
        assert!(payload.redacted);
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["target"]["id"], "thread-1");
        assert_eq!(json["trace_id"], "trace-1");
        assert_eq!(json["allowed_transitions"][0], "approve");
    }

    #[test]
    fn approval_ui_payload_limits_transitions_for_wrong_role_or_delegate() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let approval = queue
            .create(ApprovalCreate {
                id: "approval-1".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(now_ms().saturating_add(60_000)),
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-1".to_string(),
            })
            .unwrap();
        let delegated = queue
            .delegate(&approval.id, "org-1", "reviewer-1", "reviewer-2", None)
            .unwrap();
        let audit = queue.audit_events(&approval.id).unwrap();

        let wrong_role = approval_ui_payload(&delegated, &audit, "reviewer-2", "Member");
        assert_eq!(wrong_role.allowed_transitions, vec!["comment".to_string()]);
        let wrong_delegate = approval_ui_payload(&delegated, &audit, "reviewer-3", "Reviewer");
        assert_eq!(wrong_delegate.allowed_transitions, vec!["comment".to_string()]);
    }
}
