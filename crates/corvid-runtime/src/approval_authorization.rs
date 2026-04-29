//! Authorization checks for approval queue transitions.
//!
//! The queue owns persistence and audit. This module owns the security
//! decision for who may approve, deny, or delegate a pending approval.

use crate::approval_queue::ApprovalQueueRecord;
use crate::errors::RuntimeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalActorContext {
    pub actor_id: String,
    pub tenant_id: String,
    pub role: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalTransitionKind {
    Approve,
    Deny,
    Delegate,
}

pub fn authorize_approval_transition(
    approval: &ApprovalQueueRecord,
    actor: &ApprovalActorContext,
    transition: ApprovalTransitionKind,
    at_ms: u64,
) -> Result<(), RuntimeError> {
    validate_actor(actor)?;
    if actor.tenant_id != approval.tenant_id {
        return deny("approval actor tenant mismatch");
    }
    if actor.role != approval.required_role {
        return deny("approval actor role does not satisfy required role");
    }
    if actor.actor_id == approval.requester_actor_id {
        return deny("approval requester cannot approve their own request");
    }
    if let Some(delegated_to) = approval.delegated_to_actor_id.as_deref() {
        if actor.actor_id != delegated_to {
            return deny("approval is delegated to a different actor");
        }
    }
    if matches!(transition, ApprovalTransitionKind::Approve) && at_ms >= approval.expires_ms {
        return deny("approval contract expired before approval transition");
    }
    Ok(())
}

pub fn authorize_approval_delegate_target(
    approval: &ApprovalQueueRecord,
    delegated_to_actor_id: &str,
) -> Result<(), RuntimeError> {
    if delegated_to_actor_id.trim().is_empty() {
        return deny("delegated actor id must not be empty");
    }
    if delegated_to_actor_id == approval.requester_actor_id {
        return deny("approval cannot be delegated to the requester");
    }
    Ok(())
}

fn validate_actor(actor: &ApprovalActorContext) -> Result<(), RuntimeError> {
    for (label, value) in [
        ("approval actor id", actor.actor_id.as_str()),
        ("approval actor tenant", actor.tenant_id.as_str()),
        ("approval actor role", actor.role.as_str()),
    ] {
        if value.trim().is_empty() {
            return deny(&format!("{label} must not be empty"));
        }
    }
    Ok(())
}

fn deny(message: &str) -> Result<(), RuntimeError> {
    Err(RuntimeError::Other(message.to_string()))
}
