//! Catalog FFI and owned outcome types.

use crate::effect_filter::CorvidFindAgentsStatus;
use crate::errors::RuntimeError;
use std::ffi::c_char;
use std::sync::Arc;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorvidTrustTier {
    Autonomous = 0,
    HumanRequired = 1,
    SecurityReview = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CorvidAgentHandle {
    pub name: *const c_char,
    pub symbol: *const c_char,
    pub source_file: *const c_char,
    pub source_line: u32,
    pub trust_tier: u8,
    pub cost_bound_usd: f64,
    pub reversible: u8,
    pub latency_instant: u8,
    pub replayable: u8,
    pub deterministic: u8,
    pub dangerous: u8,
    pub pub_extern_c: u8,
    pub requires_approval: u8,
    pub grounded_source_count: u32,
    pub param_count: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorvidFindAgentsResult {
    pub status: CorvidFindAgentsStatus,
    pub matched_count: usize,
    pub error_message: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum CorvidCallStatus {
    Ok = 0,
    AgentNotFound = 1,
    BadArgs = 2,
    UnsupportedSig = 3,
    ApprovalRequired = 4,
    BudgetExceeded = 5,
    RuntimeError = 6,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CorvidApprovalRequired {
    pub site_name: *const c_char,
    pub predicate_json: *const c_char,
    pub args_json: *const c_char,
    pub rationale_prompt: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum CorvidPreFlightStatus {
    Ok = 0,
    AgentNotFound = 1,
    BadArgs = 2,
    UnsupportedSig = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CorvidPreFlight {
    pub status: CorvidPreFlightStatus,
    pub cost_bound_usd: f64,
    pub requires_approval: u8,
    pub effect_row_json: *const c_char,
    pub grounded_source_set_json: *const c_char,
    pub bad_args_message: *const c_char,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorvidApprovalDecision {
    Accept = 0,
    Reject = 1,
}

pub type CorvidApproverFn =
    unsafe extern "C" fn(*const CorvidApprovalRequired, *mut std::ffi::c_void) -> i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarAbiType {
    Int,
    Float,
    Bool,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarReturnType {
    Int,
    Float,
    Bool,
    String,
    Nothing,
}

pub(crate) struct ScalarInvocation {
    pub result: serde_json::Value,
    pub observation_handle: u64,
}

pub(crate) type ScalarInvoker =
    Arc<dyn Fn(&[serde_json::Value]) -> Result<ScalarInvocation, RuntimeError> + Send + Sync>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct OwnedApprovalRequired {
    pub site_name: String,
    pub predicate_json: String,
    pub args_json: String,
    pub rationale_prompt: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OwnedPreFlight {
    pub status: CorvidPreFlightStatus,
    pub cost_bound_usd: f64,
    pub requires_approval: bool,
    pub effect_row_json: String,
    pub grounded_source_set_json: String,
    pub bad_args_message: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OwnedCallOutcome {
    pub status: CorvidCallStatus,
    pub result_json: Option<String>,
    pub approval: Option<OwnedApprovalRequired>,
    pub observation_handle: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OwnedFindAgentsOutcome {
    pub status: CorvidFindAgentsStatus,
    pub matched_indices: Vec<usize>,
    pub error_message: Option<String>,
}
