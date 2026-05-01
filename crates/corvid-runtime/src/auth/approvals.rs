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
    } else if !matches!(
        contract.algorithm.as_str(),
        "RS256" | "ES256" | "EdDSA"
    ) {
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
