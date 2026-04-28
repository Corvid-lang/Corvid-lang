mod approval_contract;
mod attestation;
mod canonical_hash;
mod effect_emit;
mod embedded;
mod emit;
mod introspection_catalog;
mod provenance_emit;
mod schema;
mod signing;
mod tool_contract;
mod type_description;

pub use attestation::{
    attestation_to_embedded_bytes, parse_embedded_attestation_bytes, EmbeddedAttestationError,
    EmbeddedAttestationSection, CORVID_ABI_ATTESTATION_PAYLOAD_TYPE,
    CORVID_ABI_ATTESTATION_SECTION_MAGIC, CORVID_ABI_ATTESTATION_SYMBOL,
};
pub use canonical_hash::{hash_abi, hash_json_bytes, hash_json_str};
pub use embedded::{
    descriptor_from_embedded_section, descriptor_to_embedded_bytes, parse_embedded_section_bytes,
    read_embedded_section_from_library, EmbeddedDescriptorError, EmbeddedDescriptorSection,
    CORVID_ABI_DESCRIPTOR_SYMBOL, CORVID_ABI_SECTION_MAGIC,
};
pub use emit::{emit_abi, normalize_source_path, EmitOptions};
pub use introspection_catalog::{introspection_agents, with_introspection_agents};
pub use schema::{
    AbiAgent, AbiApprovalContract, AbiApprovalLabel, AbiApprovalSite, AbiAttributes, AbiBudget,
    AbiClaimGuarantee, AbiCostEnvelope, AbiDeclaredAt, AbiDestructor, AbiDestructorKind,
    AbiDispatch, AbiEffects, AbiField, AbiGroundedType, AbiLatencyMs, AbiListType, AbiMinExpected,
    AbiOptionType, AbiOwnership, AbiOwnershipMode, AbiParam, AbiProgressiveStage,
    AbiProjectedTokens, AbiProjectedUsd, AbiPrompt, AbiProvenanceContract, AbiResultType,
    AbiRouteArm, AbiSourceSpan, AbiStore, AbiStoreAccessor, AbiStoreAccessorKind, AbiStoreEffects,
    AbiStorePolicy, AbiTool, AbiToolContract, AbiToolDomainEffect, AbiTypeDecl, AbiVersionError,
    AbiWeakType, CorvidAbi, ScalarTypeName, TypeDescription, CORVID_ABI_VERSION,
    MIN_SUPPORTED_ABI_VERSION,
};
pub use signing::{
    load_signing_key, load_verifying_key, pae, sign_envelope, verify_envelope, DsseEnvelope,
    DsseSignature, KeySource, SignError, VerifyError,
};

use std::io;
use std::path::Path;

pub fn render_descriptor_json(abi: &CorvidAbi) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(abi)
}

pub fn descriptor_from_json(json: &str) -> Result<CorvidAbi, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn read_descriptor_from_path(path: &Path) -> Result<CorvidAbi, io::Error> {
    let json = std::fs::read_to_string(path)?;
    descriptor_from_json(&json).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

pub fn emit_catalog_abi(
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    checked: &corvid_types::Checked,
    ir: &corvid_ir::IrFile,
    registry: &corvid_types::EffectRegistry,
    opts: &EmitOptions<'_>,
) -> CorvidAbi {
    with_introspection_agents(emit_abi(file, resolved, checked, ir, registry, opts))
}
