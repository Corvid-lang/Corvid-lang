mod approval_contract;
mod effect_emit;
mod emit;
mod provenance_emit;
mod schema;
mod type_description;

pub use emit::{emit_abi, normalize_source_path, EmitOptions};
pub use schema::{
    AbiAgent, AbiApprovalContract, AbiApprovalLabel, AbiApprovalSite, AbiAttributes, AbiBudget,
    AbiCostEnvelope, AbiDeclaredAt, AbiDispatch, AbiEffects, AbiField, AbiGroundedType,
    AbiLatencyMs, AbiListType, AbiMinExpected, AbiOptionType, AbiParam, AbiProgressiveStage,
    AbiProjectedTokens, AbiProjectedUsd, AbiPrompt, AbiProvenanceContract, AbiResultType,
    AbiRouteArm, AbiSourceSpan, AbiTool, AbiTypeDecl, AbiVersionError, AbiWeakType, CorvidAbi,
    ScalarTypeName, TypeDescription, CORVID_ABI_VERSION, MIN_SUPPORTED_ABI_VERSION,
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
