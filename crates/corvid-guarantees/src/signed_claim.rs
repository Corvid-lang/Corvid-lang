//! The signed-cdylib claim whitelist — slice 35-J / signed
//! attestation surface, decomposed in Phase 20j-A8.
//!
//! Every guarantee id in [`SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS`]
//! is asserted by the build's DSSE-signed ABI descriptor for
//! every signed cdylib artifact. The build signing gate
//! (`corvid build --sign`) refuses to emit a signature unless
//! every declared contract in the source maps to a registry
//! entry whose id is in this list — so the signed binary
//! advertises only enforced claims, never aspirational ones.
//!
//! Excluded from the list:
//! - guarantees whose subject is not a cdylib (e.g.,
//!   receipt-envelope verification, observability sink shape)
//! - explicit non-defenses (the `OutOfScope` rows in
//!   [`super::registry::GUARANTEE_REGISTRY`])

use super::registry::lookup;
use super::types::Guarantee;

/// Guarantee ids carried by every signed cdylib ABI descriptor.
///
/// This list excludes guarantees whose subject is not a cdylib
/// artifact, such as receipt-envelope verification, and excludes
/// explicit non-defenses. The build signing gate checks source
/// declarations against this set before it emits a DSSE attestation.
pub const SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS: &[&str] = &[
    "approval.dangerous_call_requires_token",
    "approval.token_lexical_only",
    "approval.dangerous_marker_preserved",
    "approval.reachable_entrypoints_require_contract",
    "effect_row.body_completeness",
    "effect_row.caller_propagation",
    "effect_row.import_boundary",
    "grounded.provenance_required",
    "grounded.propagation_across_calls",
    "budget.compile_time_ceiling",
    "confidence.min_threshold",
    "replay.deterministic_pure_path",
    "abi_descriptor.cdylib_emission",
    "abi_descriptor.byte_determinism",
    "abi_descriptor.bilateral_source_match",
    "abi_attestation.envelope_signature",
    "abi_attestation.descriptor_match",
    "abi_attestation.sign_requires_claim_coverage",
    "jobs.cron_schedule_durable",
    "jobs.idempotency_key_uniqueness",
    "jobs.lease_exclusivity",
    "jobs.durable_resume",
    "jobs.cron_dst_correct",
    "auth.api_key_at_rest_hashed",
    "auth.jwt_kid_rotation",
    "auth.oauth_pkce_required",
    "connector.scope_minimum_enforced",
    "connector.rate_limit_respects_provider",
    "connector.webhook_signature_verified",
    "connector.replay_quarantine",
];

pub fn signed_cdylib_claim_guarantees() -> impl Iterator<Item = &'static Guarantee> {
    SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS
        .iter()
        .filter_map(|id| lookup(id))
}
