use crate::approval_queue::ApprovalContractRecord;
use crate::tracing::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalContractPolicyReport {
    pub contract_id: String,
    pub valid: bool,
    pub violations: Vec<String>,
}

pub fn validate_approval_contract_policy(
    contract: &ApprovalContractRecord,
) -> ApprovalContractPolicyReport {
    validate_approval_contract_policy_at(contract, now_ms())
}

pub fn validate_approval_contract_policy_at(
    contract: &ApprovalContractRecord,
    now: u64,
) -> ApprovalContractPolicyReport {
    let mut violations = Vec::new();
    if contract.action.trim().is_empty() {
        violations.push("missing_action".to_string());
    }
    if contract.target_kind.trim().is_empty() || contract.target_id.trim().is_empty() {
        violations.push("missing_target".to_string());
    }
    if contract.required_role.trim().is_empty() {
        violations.push("missing_required_role".to_string());
    }
    if !contract.max_cost_usd.is_finite() || contract.max_cost_usd < 0.0 {
        violations.push("invalid_max_cost".to_string());
    }
    if contract.data_class.trim().is_empty() {
        violations.push("missing_data_class".to_string());
    }
    if contract.expires_ms <= now {
        violations.push("expired_contract".to_string());
    }
    if contract.irreversible {
        if contract.required_role == "Member" || contract.required_role == "Service" {
            violations.push("irreversible_requires_elevated_role".to_string());
        }
        if contract.data_class == "unknown" {
            violations.push("irreversible_requires_known_data_class".to_string());
        }
        if contract.expires_ms.saturating_sub(now) > 86_400_000 {
            violations.push("irreversible_expiry_too_long".to_string());
        }
    }
    ApprovalContractPolicyReport {
        contract_id: contract.id.clone(),
        valid: violations.is_empty(),
        violations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(now: u64) -> ApprovalContractRecord {
        ApprovalContractRecord {
            id: "contract-1".to_string(),
            version: "v1".to_string(),
            action: "IssueRefund".to_string(),
            target_kind: "order".to_string(),
            target_id: "order-1".to_string(),
            tenant_id: "org-1".to_string(),
            required_role: "Reviewer".to_string(),
            max_cost_usd: 10.0,
            data_class: "financial".to_string(),
            irreversible: true,
            expires_ms: now + 60_000,
            replay_key: "replay-1".to_string(),
            created_ms: now,
        }
    }

    #[test]
    fn approval_policy_accepts_bounded_irreversible_contract() {
        let report = validate_approval_contract_policy_at(&contract(1_000), 1_000);
        assert!(report.valid);
        assert!(report.violations.is_empty());
    }

    #[test]
    fn approval_policy_rejects_weak_irreversible_contracts() {
        let now = 1_000;
        let mut bad = contract(now);
        bad.required_role = "Member".to_string();
        bad.data_class = "unknown".to_string();
        bad.expires_ms = now + 172_800_000;
        let report = validate_approval_contract_policy_at(&bad, now);
        assert!(!report.valid);
        assert!(report
            .violations
            .contains(&"irreversible_requires_elevated_role".to_string()));
        assert!(report
            .violations
            .contains(&"irreversible_requires_known_data_class".to_string()));
        assert!(report
            .violations
            .contains(&"irreversible_expiry_too_long".to_string()));
    }
}
