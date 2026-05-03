//! Pure validators for authorization-flow contracts —
//! no SQLite, no `SessionAuthRuntime` access, no side effects.
//!
//! `validate_jwt_verification_contract` enforces the production
//! constraints on a `JwtVerificationContract` (HTTPS JWKS,
//! production-grade algorithm, present subject/tenant claims,
//! bounded clock skew). `authorize_trace_permission` cross-checks
//! an `AuthActor` + `AuthTraceContext` + `PermissionRequirement`
//! and produces an `AuthorizationDecision` with a single,
//! redacted reason string.
//!
//! Both functions return diagnostics with `redacted: true` so the
//! resulting structures are safe to log without leaking the
//! underlying claim values.

use super::{
    AuthActor, AuthTraceContext, AuthorizationDecision, JwtContractDiagnostic,
    JwtVerificationContract, PermissionRequirement,
};

pub fn validate_jwt_verification_contract(
    contract: &JwtVerificationContract,
) -> JwtContractDiagnostic {
    let failure = if contract.issuer.trim().is_empty() {
        Some("missing_issuer")
    } else if contract.audience.trim().is_empty() {
        Some("missing_audience")
    } else if contract.jwks_url.trim().is_empty() {
        Some("missing_jwks_url")
    } else if !(contract.jwks_url.starts_with("https://")
        || contract.jwks_url.starts_with("http://localhost")
        || contract.jwks_url.starts_with("http://127.0.0.1"))
    {
        Some("jwks_url_not_https")
    } else if !matches!(contract.algorithm.as_str(), "RS256" | "ES256" | "EdDSA") {
        Some("unsupported_algorithm")
    } else if contract.required_subject_claim.trim().is_empty() {
        Some("missing_subject_claim")
    } else if contract.required_tenant_claim.trim().is_empty() {
        Some("missing_tenant_claim")
    } else if contract.clock_skew_ms > 300_000 {
        Some("clock_skew_too_large")
    } else {
        None
    };
    JwtContractDiagnostic {
        valid: failure.is_none(),
        failure_kind: failure.map(str::to_string),
        redacted: true,
    }
}

pub fn authorize_trace_permission(
    actor: &AuthActor,
    trace: &AuthTraceContext,
    requirement: &PermissionRequirement,
) -> AuthorizationDecision {
    let reason = if actor.tenant_id != requirement.tenant_id {
        Some("actor tenant does not match requirement tenant")
    } else if trace.tenant_id != requirement.tenant_id {
        Some("trace tenant does not match requirement tenant")
    } else if trace.actor_id != actor.id {
        Some("trace actor does not match actor")
    } else if trace.trace_id != requirement.trace_id {
        Some("trace id does not match requirement")
    } else if actor.permission_fingerprint != requirement.permission_fingerprint {
        Some("actor permission fingerprint does not satisfy requirement")
    } else if trace.permission_fingerprint != requirement.permission_fingerprint {
        Some("trace permission fingerprint does not satisfy requirement")
    } else {
        None
    };
    AuthorizationDecision {
        allowed: reason.is_none(),
        actor_id: actor.id.clone(),
        tenant_id: requirement.tenant_id.clone(),
        permission: requirement.permission.clone(),
        surface_kind: requirement.surface_kind.clone(),
        surface_id: requirement.surface_id.clone(),
        trace_id: requirement.trace_id.clone(),
        reason: reason.unwrap_or("permission propagated").to_string(),
        redacted: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: &str, tenant_id: &str) -> AuthActor {
        AuthActor {
            id: id.to_string(),
            tenant_id: tenant_id.to_string(),
            display_name: "Ada".to_string(),
            actor_kind: "user".to_string(),
            auth_method: "session".to_string(),
            assurance_level: "aal1".to_string(),
            role_fingerprint: "sha256:roles".to_string(),
            permission_fingerprint: "sha256:permissions".to_string(),
            created_ms: 1,
            updated_ms: 1,
        }
    }

    #[test]
    fn jwt_contract_validation_accepts_production_algorithms_and_redacts_failures() {
        let contract = JwtVerificationContract {
            issuer: "https://issuer.example".to_string(),
            audience: "corvid-api".to_string(),
            jwks_url: "https://issuer.example/.well-known/jwks.json".to_string(),
            algorithm: "RS256".to_string(),
            required_tenant_claim: "tenant".to_string(),
            required_subject_claim: "sub".to_string(),
            clock_skew_ms: 60_000,
        };
        let ok = validate_jwt_verification_contract(&contract);
        assert!(ok.valid);
        assert_eq!(ok.failure_kind, None);
        assert!(ok.redacted);

        for (algorithm, failure) in [
            ("none", "unsupported_algorithm"),
            ("HS256", "unsupported_algorithm"),
        ] {
            let mut bad = contract.clone();
            bad.algorithm = algorithm.to_string();
            let diagnostic = validate_jwt_verification_contract(&bad);
            assert!(!diagnostic.valid);
            assert_eq!(diagnostic.failure_kind.as_deref(), Some(failure));
            assert!(diagnostic.redacted);
        }

        let mut insecure = contract.clone();
        insecure.jwks_url = "http://issuer.example/jwks.json".to_string();
        let diagnostic = validate_jwt_verification_contract(&insecure);
        assert!(!diagnostic.valid);
        assert_eq!(
            diagnostic.failure_kind.as_deref(),
            Some("jwks_url_not_https")
        );
        assert!(diagnostic.redacted);
    }

    #[test]
    fn permission_propagation_binds_actor_tenant_trace_and_surface() {
        let actor = actor("user-1", "org-1");
        let trace = AuthTraceContext {
            trace_id: "trace-1".to_string(),
            tenant_id: "org-1".to_string(),
            actor_id: "user-1".to_string(),
            auth_method: "session".to_string(),
            session_id: "sess-1".to_string(),
            api_key_id: String::new(),
            permission_fingerprint: "sha256:permissions".to_string(),
            replay_key: "replay-1".to_string(),
        };
        let requirement = PermissionRequirement {
            tenant_id: "org-1".to_string(),
            permission: "CanReviewEmail".to_string(),
            permission_fingerprint: "sha256:permissions".to_string(),
            surface_kind: "job".to_string(),
            surface_id: "email_triage_job".to_string(),
            trace_id: "trace-1".to_string(),
        };
        let allowed = authorize_trace_permission(&actor, &trace, &requirement);
        assert!(allowed.allowed);
        assert_eq!(allowed.surface_kind, "job");
        assert_eq!(allowed.reason, "permission propagated");
        assert!(allowed.redacted);

        let mut cross_tenant = requirement.clone();
        cross_tenant.tenant_id = "org-2".to_string();
        let denied = authorize_trace_permission(&actor, &trace, &cross_tenant);
        assert!(!denied.allowed);
        assert!(denied.reason.contains("tenant"));

        let mut stale_trace = trace.clone();
        stale_trace.permission_fingerprint = "sha256:old".to_string();
        let denied = authorize_trace_permission(&actor, &stale_trace, &requirement);
        assert!(!denied.allowed);
        assert!(denied.reason.contains("trace permission"));
    }
}
