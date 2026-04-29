use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CORVID_ABI_VERSION: u32 = 1;
pub const MIN_SUPPORTED_ABI_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiVersionError {
    Unsupported { found: u32, min: u32, max: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorvidAbi {
    pub corvid_abi_version: u32,
    pub compiler_version: String,
    pub source_path: String,
    pub generated_at: String,
    #[serde(default)]
    pub agents: Vec<AbiAgent>,
    #[serde(default)]
    pub prompts: Vec<AbiPrompt>,
    #[serde(default)]
    pub tools: Vec<AbiTool>,
    #[serde(default)]
    pub types: Vec<AbiTypeDecl>,
    #[serde(default)]
    pub stores: Vec<AbiStore>,
    #[serde(default)]
    pub approval_sites: Vec<AbiApprovalSite>,
    #[serde(default)]
    pub claim_guarantees: Vec<AbiClaimGuarantee>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

impl CorvidAbi {
    pub fn validate_supported_version(&self) -> Result<(), AbiVersionError> {
        if (MIN_SUPPORTED_ABI_VERSION..=CORVID_ABI_VERSION).contains(&self.corvid_abi_version) {
            Ok(())
        } else {
            Err(AbiVersionError::Unsupported {
                found: self.corvid_abi_version,
                min: MIN_SUPPORTED_ABI_VERSION,
                max: CORVID_ABI_VERSION,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiClaimGuarantee {
    pub id: String,
    pub kind: String,
    pub class: String,
    pub phase: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiSourceSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiAgent {
    pub name: String,
    pub symbol: String,
    pub source_span: AbiSourceSpan,
    #[serde(default)]
    pub source_line: u32,
    #[serde(default)]
    pub params: Vec<AbiParam>,
    pub return_type: TypeDescription,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_ownership: Option<AbiOwnership>,
    pub effects: AbiEffects,
    pub attributes: AbiAttributes,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<AbiBudget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<AbiDispatch>,
    pub approval_contract: AbiApprovalContract,
    pub provenance: AbiProvenanceContract,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiPrompt {
    pub name: String,
    pub source_span: AbiSourceSpan,
    #[serde(default)]
    pub params: Vec<AbiParam>,
    pub return_type: TypeDescription,
    pub effects: AbiEffects,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<AbiDispatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_envelope: Option<AbiCostEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_floor: Option<f64>,
    #[serde(default)]
    pub cited_params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiTool {
    pub name: String,
    pub symbol: String,
    #[serde(default)]
    pub params: Vec<AbiParam>,
    pub return_type: TypeDescription,
    pub effects: AbiEffects,
    pub dangerous: bool,
    #[serde(default, skip_serializing_if = "AbiToolContract::is_empty")]
    pub contract: AbiToolContract,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AbiToolContract {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_effects: Vec<AbiToolDomainEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_approval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_approval: Option<AbiGeneratedApprovalContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_card_hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ci_fail_on: Vec<String>,
}

impl AbiToolContract {
    pub fn is_empty(&self) -> bool {
        self.domain_effects.is_empty()
            && self.requires_approval.is_none()
            && self.generated_approval.is_none()
            && self.approval_card_hints.is_empty()
            && self.ci_fail_on.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiGeneratedApprovalContract {
    pub id: String,
    pub version: String,
    pub expected_action: String,
    pub target_resource: String,
    pub max_cost_usd: f64,
    pub data_touched: String,
    pub irreversible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry_ms: Option<u64>,
    pub required_role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiToolDomainEffect {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub source_effect: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiTypeDecl {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub fields: Vec<AbiField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiStore {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub fields: Vec<AbiField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<AbiStorePolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accessors: Vec<AbiStoreAccessor>,
    pub source_span: AbiSourceSpan,
    pub effects: AbiStoreEffects,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiStorePolicy {
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiStoreEffects {
    pub read: String,
    pub write: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbiStoreAccessorKind {
    Get,
    Set,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiStoreAccessor {
    pub name: String,
    pub field: String,
    pub kind: AbiStoreAccessorKind,
    pub value_type: TypeDescription,
    pub effect: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiField {
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: TypeDescription,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiApprovalSite {
    pub label: String,
    pub declared_at: AbiDeclaredAt,
    pub agent_context: String,
    #[serde(default)]
    pub predicate: Option<Value>,
    #[serde(default)]
    pub dangerous_targets: Vec<String>,
    pub effects: AbiEffects,
    pub required_tier: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiDeclaredAt {
    pub source_span: AbiSourceSpan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiParam {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: TypeDescription,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership: Option<AbiOwnership>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbiOwnershipMode {
    Owned,
    Borrowed,
    Shared,
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbiDestructorKind {
    Drop,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiDestructor {
    pub kind: AbiDestructorKind,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiOwnership {
    pub mode: AbiOwnershipMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructor: Option<AbiDestructor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypeDescription {
    Scalar {
        scalar: ScalarTypeName,
    },
    Struct {
        #[serde(rename = "struct")]
        name: String,
    },
    List {
        list: AbiListType,
    },
    Result {
        result: AbiResultType,
    },
    Option {
        option: AbiOptionType,
    },
    Grounded {
        grounded: AbiGroundedType,
    },
    Partial {
        partial: AbiPartialType,
    },
    ResumeToken {
        resume_token: AbiResumeTokenType,
    },
    Weak {
        weak: AbiWeakType,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScalarTypeName {
    Int,
    Float,
    String,
    Bool,
    Nothing,
    TraceId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiListType {
    pub element: Box<TypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiResultType {
    pub ok: Box<TypeDescription>,
    pub err: Box<TypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiOptionType {
    pub inner: Box<TypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiGroundedType {
    pub inner: Box<TypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiPartialType {
    pub inner: Box<TypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiResumeTokenType {
    pub inner: Box<TypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiWeakType {
    pub inner: Box<TypeDescription>,
    #[serde(default)]
    pub effects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AbiEffects {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<AbiProjectedUsd>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<AbiLatencyMs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<AbiMinExpected>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<AbiProjectedTokens>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub custom: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiProjectedUsd {
    pub projected_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiLatencyMs {
    pub p99_estimate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiMinExpected {
    pub min_expected: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiProjectedTokens {
    pub projected: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AbiAttributes {
    pub replayable: bool,
    pub deterministic: bool,
    pub dangerous: bool,
    pub pub_extern_c: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiBudget {
    pub usd_per_call: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiApprovalContract {
    pub required: bool,
    #[serde(default)]
    pub labels: Vec<AbiApprovalLabel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiApprovalLabel {
    pub label: String,
    #[serde(default)]
    pub args: Vec<AbiParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_at_site: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_tier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiProvenanceContract {
    pub returns_grounded: bool,
    #[serde(default)]
    pub grounded_param_deps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AbiDispatch {
    Route {
        #[serde(default)]
        route_arms: Vec<AbiRouteArm>,
    },
    Progressive {
        #[serde(default)]
        stages: Vec<AbiProgressiveStage>,
    },
    Rollout {
        variant: String,
        baseline: String,
        #[serde(rename = "variant_percent")]
        variant_percent: f64,
    },
    Ensemble {
        #[serde(default)]
        models: Vec<String>,
        vote_strategy: String,
    },
    Adversarial {
        #[serde(rename = "propose")]
        propose: String,
        #[serde(rename = "challenge")]
        challenge: String,
        #[serde(rename = "adjudicate")]
        adjudicate: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiRouteArm {
    pub model: String,
    pub matcher: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiProgressiveStage {
    pub model_requires: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub escalate_below_confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbiCostEnvelope {
    pub min_usd: f64,
    pub typical_usd: f64,
    pub max_usd: f64,
}
