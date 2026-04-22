use crate::effect_filter::{self, CorvidFindAgentsStatus, FilterAgent};
use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use corvid_abi::{
    descriptor_from_embedded_section, AbiAgent, AbiApprovalLabel, AbiApprovalSite, ScalarTypeName,
    TypeDescription,
};
use corvid_trace_schema::TraceEvent;
use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::sync::{Arc, OnceLock};

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

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorvidApprovalDecision {
    Accept = 0,
    Reject = 1,
}

pub type CorvidApproverFn =
    unsafe extern "C" fn(*const CorvidApprovalRequired, *mut std::ffi::c_void) -> CorvidApprovalDecision;

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

pub(crate) type ScalarInvoker =
    Arc<dyn Fn(&[serde_json::Value]) -> Result<serde_json::Value, RuntimeError> + Send + Sync>;

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
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OwnedFindAgentsOutcome {
    pub status: CorvidFindAgentsStatus,
    pub matched_indices: Vec<usize>,
    pub error_message: Option<String>,
}

enum CatalogInvoker {
    Introspection(IntrospectionKind),
    Scalar(ScalarInvoker),
    Unsupported { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntrospectionKind {
    DescriptorJson,
    Verify,
    ListAgents,
    AgentSignatureJson,
    PreFlight,
    CallAgent,
    FindAgentsWhere,
}

struct AgentCatalogEntry {
    abi: AbiAgent,
    name_c: CString,
    symbol_c: CString,
    source_file_c: CString,
    signature_json_c: CString,
    effect_row_json: String,
    grounded_source_set_json: String,
    invoker: CatalogInvoker,
}

struct CatalogState {
    descriptor_json: String,
    descriptor_json_c: CString,
    descriptor_hash: [u8; 32],
    approval_sites: Vec<AbiApprovalSite>,
    agents: Vec<AgentCatalogEntry>,
    by_name: HashMap<String, usize>,
}

struct CatalogInit(Result<CatalogState, RuntimeError>);

static CATALOG: OnceLock<CatalogInit> = OnceLock::new();

pub fn descriptor_json() -> Result<(&'static str, usize), RuntimeError> {
    let state = catalog()?;
    Ok((&state.descriptor_json, state.descriptor_json.len()))
}

pub fn descriptor_json_ptr() -> Result<(*const c_char, usize), RuntimeError> {
    let state = catalog()?;
    Ok((state.descriptor_json_c.as_ptr(), state.descriptor_json.len()))
}

pub fn descriptor_hash() -> Result<[u8; 32], RuntimeError> {
    Ok(catalog()?.descriptor_hash)
}

pub fn verify_hash(expected: &[u8; 32]) -> Result<bool, RuntimeError> {
    let actual = descriptor_hash()?;
    let mut diff = 0u8;
    for (left, right) in actual.iter().zip(expected.iter()) {
        diff |= left ^ right;
    }
    Ok(diff == 0)
}

pub fn list_agents() -> Result<&'static [CorvidAgentHandle], RuntimeError> {
    let _ = catalog()?;
    Ok(&[])
}

pub(crate) fn list_agent_handles_owned() -> Result<Vec<CorvidAgentHandle>, RuntimeError> {
    let state = catalog()?;
    let mut handles = Vec::with_capacity(state.agents.len() + 1);
    for entry in state.agents.iter().filter(|entry| is_introspection_agent(&entry.abi.name)) {
        handles.push(handle_from_entry(entry));
    }
    if let Some(overlay) = crate::approver_bridge::registered_approver_overlay() {
        handles.push(CorvidAgentHandle {
            name: overlay.name_ptr,
            symbol: overlay.symbol_ptr,
            source_file: overlay.source_file_ptr,
            source_line: overlay.abi.source_line,
            trust_tier: CorvidTrustTier::Autonomous as u8,
            cost_bound_usd: overlay.display_budget_usd,
            reversible: 1,
            latency_instant: overlay
                .abi
                .effects
                .latency_ms
                .as_ref()
                .map(|latency| latency.p99_estimate <= 1.0)
                .unwrap_or(false) as u8,
            replayable: overlay.abi.attributes.replayable as u8,
            deterministic: overlay.abi.attributes.deterministic as u8,
            dangerous: 0,
            pub_extern_c: 0,
            requires_approval: 0,
            grounded_source_count: 0,
            param_count: overlay.abi.params.len() as u32,
        });
    }
    for entry in state
        .agents
        .iter()
        .filter(|entry| !is_introspection_agent(&entry.abi.name))
    {
        handles.push(handle_from_entry(entry));
    }
    Ok(handles)
}

pub(crate) fn catalog_approval_sites() -> Result<Vec<AbiApprovalSite>, RuntimeError> {
    Ok(catalog()?.approval_sites.clone())
}

pub fn agent_signature_json(
    name: &str,
) -> Result<Option<(&'static str, usize, *const c_char)>, RuntimeError> {
    if name == "__corvid_approver" {
        return Ok(crate::approver_bridge::registered_approver_overlay().map(|overlay| {
            let value: &'static str = Box::leak(overlay.signature_json.into_boxed_str());
            (value, overlay.signature_json_len, overlay.signature_json_ptr)
        }));
    }
    let state = catalog()?;
    let Some(entry) = state.by_name.get(name).and_then(|idx| state.agents.get(*idx)) else {
        return Ok(None);
    };
    let value = entry
        .signature_json_c
        .to_str()
        .map_err(|err| RuntimeError::Other(format!("catalog signature UTF-8 bug: {err}")))?;
    Ok(Some((value, value.len(), entry.signature_json_c.as_ptr())))
}

pub fn pre_flight(agent_name: &str, args_json: &str) -> OwnedPreFlight {
    match pre_flight_impl(agent_name, args_json) {
        Ok(value) => value,
        Err(err) => OwnedPreFlight {
            status: CorvidPreFlightStatus::BadArgs,
            cost_bound_usd: f64::NAN,
            requires_approval: false,
            effect_row_json: String::new(),
            grounded_source_set_json: String::new(),
            bad_args_message: Some(err.to_string()),
        },
    }
}

pub fn call_agent(agent_name: &str, args_json: &str) -> OwnedCallOutcome {
    match call_agent_impl(agent_name, args_json) {
        Ok(outcome) => outcome,
        Err(err) => OwnedCallOutcome {
            status: CorvidCallStatus::RuntimeError,
            result_json: Some(serde_json::json!({ "error": err.to_string() }).to_string()),
            approval: None,
        },
    }
}

pub fn find_agents_where(filter_json: &str) -> OwnedFindAgentsOutcome {
    match find_agents_where_impl(filter_json) {
        Ok(outcome) => outcome,
        Err(err) => OwnedFindAgentsOutcome {
            status: CorvidFindAgentsStatus::BadJson,
            matched_indices: Vec::new(),
            error_message: Some(err.to_string()),
        },
    }
}

fn catalog() -> Result<&'static CatalogState, RuntimeError> {
    let init = CATALOG.get_or_init(|| CatalogInit(load_catalog()));
    match &init.0 {
        Ok(state) => Ok(state),
        Err(err) => Err(err.clone()),
    }
}

fn load_catalog() -> Result<CatalogState, RuntimeError> {
    let section = crate::catalog_c_api::load_embedded_descriptor_from_current_library()?;
    let descriptor = descriptor_from_embedded_section(&section)
        .map_err(|err| RuntimeError::Other(format!("parse embedded descriptor: {err}")))?;
    let descriptor_json = section.json;
    let descriptor_json_c = CString::new(descriptor_json.clone())
        .map_err(|err| RuntimeError::Other(format!("descriptor JSON contained NUL: {err}")))?;
    let source_path = descriptor.source_path.clone();
    let approval_sites = descriptor.approval_sites.clone();

    let mut agents = Vec::with_capacity(descriptor.agents.len());
    let mut by_name = HashMap::with_capacity(descriptor.agents.len());
    for abi in descriptor.agents {
        let signature_json = serde_json::to_string_pretty(&abi)
            .map_err(|err| RuntimeError::Other(format!("serialize agent signature: {err}")))?;
        let signature_json_c = CString::new(signature_json)
            .map_err(|err| RuntimeError::Other(format!("agent signature contained NUL: {err}")))?;
        let name_c = cstring(&abi.name)?;
        let symbol_c = cstring(&abi.symbol)?;
        let source_file_c = cstring(&source_path)?;
        let effect_row_json = serde_json::to_string(&abi.effects)
            .map_err(|err| RuntimeError::Other(format!("serialize effect row: {err}")))?;
        let grounded_source_set_json = serde_json::to_string(&abi.provenance.grounded_param_deps)
            .map_err(|err| RuntimeError::Other(format!("serialize grounded source set: {err}")))?;
        let invoker = build_invoker(&abi)?;
        by_name.insert(abi.name.clone(), agents.len());
        agents.push(AgentCatalogEntry {
            abi,
            name_c,
            symbol_c,
            source_file_c,
            signature_json_c,
            effect_row_json,
            grounded_source_set_json,
            invoker,
        });
    }

    Ok(CatalogState {
        descriptor_json,
        descriptor_json_c,
        descriptor_hash: section.sha256,
        approval_sites,
        agents,
        by_name,
    })
}

fn pre_flight_impl(agent_name: &str, args_json: &str) -> Result<OwnedPreFlight, RuntimeError> {
    if agent_name == "__corvid_approver" {
        return Ok(OwnedPreFlight {
            status: CorvidPreFlightStatus::UnsupportedSig,
            cost_bound_usd: f64::NAN,
            requires_approval: false,
            effect_row_json: String::new(),
            grounded_source_set_json: String::new(),
            bad_args_message: Some(
                "`__corvid_approver` is a governance capability and is not directly callable"
                    .to_string(),
            ),
        });
    }
    let state = catalog()?;
    let Some(entry) = state.by_name.get(agent_name).and_then(|idx| state.agents.get(*idx)) else {
        return Ok(OwnedPreFlight {
            status: CorvidPreFlightStatus::AgentNotFound,
            cost_bound_usd: f64::NAN,
            requires_approval: false,
            effect_row_json: String::new(),
            grounded_source_set_json: String::new(),
            bad_args_message: None,
        });
    };
    if matches!(entry.invoker, CatalogInvoker::Unsupported { .. }) {
        return Ok(OwnedPreFlight {
            status: CorvidPreFlightStatus::UnsupportedSig,
            cost_bound_usd: f64::NAN,
            requires_approval: false,
            effect_row_json: entry.effect_row_json.clone(),
            grounded_source_set_json: entry.grounded_source_set_json.clone(),
            bad_args_message: Some(unsupported_message(entry)),
        });
    }
    let validated = match validate_args_for_entry(entry, args_json) {
        Ok(validated) => validated,
        Err(err) => {
            return Ok(OwnedPreFlight {
                status: CorvidPreFlightStatus::BadArgs,
                cost_bound_usd: f64::NAN,
                requires_approval: false,
                effect_row_json: String::new(),
                grounded_source_set_json: String::new(),
                bad_args_message: Some(err),
            });
        }
    };
    let _ = validated;
    Ok(OwnedPreFlight {
        status: CorvidPreFlightStatus::Ok,
        cost_bound_usd: cost_bound_for(&entry.abi),
        requires_approval: entry.abi.approval_contract.required,
        effect_row_json: entry.effect_row_json.clone(),
        grounded_source_set_json: entry.grounded_source_set_json.clone(),
        bad_args_message: None,
    })
}

fn call_agent_impl(agent_name: &str, args_json: &str) -> Result<OwnedCallOutcome, RuntimeError> {
    if agent_name == "__corvid_approver" {
        return Ok(OwnedCallOutcome {
            status: CorvidCallStatus::UnsupportedSig,
            result_json: Some(
                serde_json::json!({
                    "error": "`__corvid_approver` is a governance capability and is not directly callable",
                })
                .to_string(),
            ),
            approval: None,
        });
    }
    let state = catalog()?;
    let Some(entry) = state.by_name.get(agent_name).and_then(|idx| state.agents.get(*idx)) else {
        return Ok(OwnedCallOutcome {
            status: CorvidCallStatus::AgentNotFound,
            result_json: None,
            approval: None,
        });
    };
    if let CatalogInvoker::Unsupported { .. } = &entry.invoker {
        return Ok(OwnedCallOutcome {
            status: CorvidCallStatus::UnsupportedSig,
            result_json: None,
            approval: None,
        });
    }
    let validated = match validate_args_for_entry(entry, args_json) {
        Ok(validated) => validated,
        Err(err) => {
            return Ok(OwnedCallOutcome {
                status: CorvidCallStatus::BadArgs,
                result_json: Some(serde_json::json!({ "error": err }).to_string()),
                approval: None,
            });
        }
    };

    if entry.abi.approval_contract.required {
        let approval = build_approval_required(entry, args_json)?;
        match crate::catalog_c_api::request_host_approval(&approval) {
            crate::catalog_c_api::ApprovalRequestOutcome::MissingOrRejected => {
                emit_embedded_rejected_approval(&approval, &validated.args);
                return Ok(OwnedCallOutcome {
                    status: CorvidCallStatus::ApprovalRequired,
                    result_json: None,
                    approval: Some(approval),
                });
            }
            crate::catalog_c_api::ApprovalRequestOutcome::Accepted(detail) => {
                crate::catalog_c_api::mark_preapproved_request(
                    approval.site_name.clone(),
                    validated.args.clone(),
                    detail,
                );
            }
        }
    }

    let result = match &entry.invoker {
        CatalogInvoker::Introspection(kind) => introspection_call(*kind, validated.args)?,
        CatalogInvoker::Scalar(invoker) => (invoker)(&validated.args)?,
        CatalogInvoker::Unsupported { .. } => unreachable!(),
    };
    let result_json = serde_json::to_string(&result)
        .map_err(|err| RuntimeError::Marshal(format!("serialize agent result: {err}")))?;
    Ok(OwnedCallOutcome {
        status: CorvidCallStatus::Ok,
        result_json: Some(result_json),
        approval: None,
    })
}

fn find_agents_where_impl(filter_json: &str) -> Result<OwnedFindAgentsOutcome, RuntimeError> {
    let state = catalog()?;
    let mut agents = Vec::with_capacity(state.agents.len() + 1);
    for entry in state.agents.iter().filter(|entry| is_introspection_agent(&entry.abi.name)) {
        agents.push(filter_agent_from_entry(entry, None));
    }
    if let Some(overlay) = crate::approver_bridge::registered_approver_overlay() {
        agents.push(FilterAgent {
            abi: overlay.abi,
            cost_bound_usd: finite_option(overlay.display_budget_usd),
        });
    }
    for entry in state
        .agents
        .iter()
        .filter(|entry| !is_introspection_agent(&entry.abi.name))
    {
        agents.push(filter_agent_from_entry(entry, None));
    }
    let result = effect_filter::find_matching_indices(&agents, filter_json);
    Ok(OwnedFindAgentsOutcome {
        status: result.status,
        matched_indices: result.matched_indices,
        error_message: result.error_message,
    })
}

struct ValidatedArgs {
    args: Vec<serde_json::Value>,
}

fn validate_args_for_entry(entry: &AgentCatalogEntry, args_json: &str) -> Result<ValidatedArgs, String> {
    let value: serde_json::Value = serde_json::from_str(args_json)
        .map_err(|err| format!("args_json must be a JSON array: {err}"))?;
    let serde_json::Value::Array(args) = value else {
        return Err("args_json must be a JSON array".to_string());
    };
    if args.len() != entry.abi.params.len() {
        return Err(format!(
            "arity mismatch for `{}`: expected {}, got {}",
            entry.abi.name,
            entry.abi.params.len(),
            args.len()
        ));
    }
    for (index, (param, arg)) in entry.abi.params.iter().zip(args.iter()).enumerate() {
        validate_scalar_argument(&param.ty, arg).map_err(|message| {
            format!(
                "argument {} (`{}`) for `{}` is invalid: {message}",
                index + 1,
                param.name,
                entry.abi.name
            )
        })?;
    }
    Ok(ValidatedArgs { args })
}

fn validate_scalar_argument(ty: &TypeDescription, value: &serde_json::Value) -> Result<(), String> {
    match scalar_param_type_from_descriptor(ty) {
        Ok(ScalarAbiType::Int) => value
            .as_i64()
            .map(|_| ())
            .ok_or_else(|| "expected Int".to_string()),
        Ok(ScalarAbiType::Float) => value
            .as_f64()
            .map(|_| ())
            .ok_or_else(|| "expected Float".to_string()),
        Ok(ScalarAbiType::Bool) => value
            .as_bool()
            .map(|_| ())
            .ok_or_else(|| "expected Bool".to_string()),
        Ok(ScalarAbiType::String) => value
            .as_str()
            .map(|_| ())
            .ok_or_else(|| "expected String".to_string()),
        Err(message) => Err(message),
    }
}

fn build_invoker(abi: &AbiAgent) -> Result<CatalogInvoker, RuntimeError> {
    if let Some(kind) = introspection_kind(&abi.name) {
        return Ok(CatalogInvoker::Introspection(kind));
    }
    if !abi.attributes.pub_extern_c {
        return Ok(CatalogInvoker::Unsupported {
            message: format!(
                "agent `{}` is not `pub extern \"c\"`; generic host dispatch is limited to exported scalar agents in Phase 22-C",
                abi.name
            ),
        });
    }
    let params = abi
        .params
        .iter()
        .map(|param| scalar_param_type_from_descriptor(&param.ty))
        .collect::<Result<Vec<_>, _>>();
    let ret = scalar_return_type_from_descriptor(&abi.return_type);
    match (params, ret) {
        (Ok(params), Ok(ret)) => Ok(CatalogInvoker::Scalar(
            crate::catalog_c_api::build_scalar_invoker(&abi.symbol, &params, ret)?,
        )),
        (Err(message), _) | (_, Err(message)) => Ok(CatalogInvoker::Unsupported { message }),
    }
}

fn build_approval_required(
    entry: &AgentCatalogEntry,
    args_json: &str,
) -> Result<OwnedApprovalRequired, RuntimeError> {
    let site = entry
        .abi
        .approval_contract
        .labels
        .first()
        .cloned()
        .unwrap_or_else(|| AbiApprovalLabel {
            label: entry.abi.name.clone(),
            args: Vec::new(),
            cost_at_site: None,
            reversibility: None,
            required_tier: entry.abi.effects.trust_tier.clone(),
        });
    let predicate_json = serde_json::to_string(&serde_json::json!({
        "kind": "approval_contract",
        "required": entry.abi.approval_contract.required,
        "labels": entry.abi.approval_contract.labels,
    }))
    .map_err(|err| RuntimeError::Marshal(format!("serialize approval predicate: {err}")))?;
    Ok(OwnedApprovalRequired {
        site_name: site.label,
        predicate_json,
        args_json: args_json.to_string(),
        rationale_prompt: format!(
            "Agent `{}` requires approval before executing dangerous effects.",
            entry.abi.name
        ),
    })
}

fn emit_embedded_rejected_approval(
    approval: &OwnedApprovalRequired,
    args: &[serde_json::Value],
) {
    crate::ffi_bridge::corvid_runtime_embed_init_default();
    let runtime = crate::ffi_bridge::runtime();
    let tracer = runtime.tracer();
    if !tracer.is_enabled() {
        return;
    }
    let run_id = tracer.run_id().to_string();
    tracer.emit(TraceEvent::ApprovalRequest {
        ts_ms: now_ms(),
        run_id: run_id.clone(),
        label: approval.site_name.clone(),
        args: args.to_vec(),
    });
    let detail = crate::catalog_c_api::take_last_approval_detail().unwrap_or(
        crate::approver_bridge::ApprovalDecisionInfo {
            accepted: false,
            decider: "fail-closed-default".to_string(),
            rationale: None,
        },
    );
    tracer.emit(TraceEvent::ApprovalDecision {
        ts_ms: now_ms(),
        run_id: run_id.clone(),
        site: approval.site_name.clone(),
        args: args.to_vec(),
        accepted: detail.accepted,
        decider: detail.decider,
        rationale: detail.rationale,
    });
    tracer.emit(TraceEvent::ApprovalResponse {
        ts_ms: now_ms(),
        run_id,
        label: approval.site_name.clone(),
        approved: false,
    });
}

fn introspection_call(
    kind: IntrospectionKind,
    args: Vec<serde_json::Value>,
) -> Result<serde_json::Value, RuntimeError> {
    match kind {
        IntrospectionKind::DescriptorJson => Ok(serde_json::Value::String(descriptor_json()?.0.to_string())),
        IntrospectionKind::Verify => {
            let expected = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| RuntimeError::Marshal("`__corvid_abi_verify` expects one String hash argument".to_string()))?;
            let expected = decode_hex_hash(expected)?;
            Ok(serde_json::Value::Bool(verify_hash(&expected)?))
        }
        IntrospectionKind::ListAgents => {
            let state = catalog()?;
            let mut agents = state
                .agents
                .iter()
                .filter(|entry| is_introspection_agent(&entry.abi.name))
                .map(|entry| serde_json::to_value(&entry.abi))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| RuntimeError::Marshal(format!("serialize agent catalog: {err}")))?;
            if let Some(overlay) = crate::approver_bridge::registered_approver_overlay() {
                agents.push(
                    serde_json::to_value(&overlay.abi)
                        .map_err(|err| RuntimeError::Marshal(format!("serialize approver overlay: {err}")))?,
                );
            }
            let mut user_agents = state
                .agents
                .iter()
                .filter(|entry| !is_introspection_agent(&entry.abi.name))
                .map(|entry| serde_json::to_value(&entry.abi))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| RuntimeError::Marshal(format!("serialize agent catalog: {err}")))?;
            agents.append(&mut user_agents);
            Ok(serde_json::Value::String(
                serde_json::to_string(&agents)
                    .map_err(|err| RuntimeError::Marshal(format!("serialize agent list JSON: {err}")))?,
            ))
        }
        IntrospectionKind::AgentSignatureJson => {
            let name = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| RuntimeError::Marshal("`__corvid_agent_signature_json` expects one String argument".to_string()))?;
            let signature = agent_signature_json(name)?
                .map(|(json, _, _)| json.to_string())
                .unwrap_or_default();
            Ok(serde_json::Value::String(signature))
        }
        IntrospectionKind::PreFlight => {
            let agent = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| RuntimeError::Marshal("`__corvid_pre_flight` expects agent name".to_string()))?;
            let args_json = args
                .get(1)
                .and_then(|value| value.as_str())
                .ok_or_else(|| RuntimeError::Marshal("`__corvid_pre_flight` expects args JSON".to_string()))?;
            Ok(serde_json::Value::String(
                serde_json::to_string(&pre_flight(agent, args_json))
                    .map_err(|err| RuntimeError::Marshal(format!("serialize preflight JSON: {err}")))?,
            ))
        }
        IntrospectionKind::CallAgent => {
            let agent = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| RuntimeError::Marshal("`__corvid_call_agent` expects agent name".to_string()))?;
            let args_json = args
                .get(1)
                .and_then(|value| value.as_str())
                .ok_or_else(|| RuntimeError::Marshal("`__corvid_call_agent` expects args JSON".to_string()))?;
            Ok(serde_json::Value::String(
                serde_json::to_string(&call_agent(agent, args_json))
                    .map_err(|err| RuntimeError::Marshal(format!("serialize call outcome JSON: {err}")))?,
            ))
        }
        IntrospectionKind::FindAgentsWhere => {
            let filter_json = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RuntimeError::Marshal(
                        "`__corvid_find_agents_where` expects one String filter argument"
                            .to_string(),
                    )
                })?;
            Ok(serde_json::Value::String(
                serde_json::to_string(&find_agents_where(filter_json)).map_err(|err| {
                    RuntimeError::Marshal(format!("serialize filter outcome JSON: {err}"))
                })?,
            ))
        }
    }
}

fn cstring(value: &str) -> Result<CString, RuntimeError> {
    CString::new(value).map_err(|err| RuntimeError::Other(format!("catalog string contained NUL: {err}")))
}

fn handle_from_entry(entry: &AgentCatalogEntry) -> CorvidAgentHandle {
    CorvidAgentHandle {
        name: entry.name_c.as_ptr(),
        symbol: entry.symbol_c.as_ptr(),
        source_file: entry.source_file_c.as_ptr(),
        source_line: entry.abi.source_line,
        trust_tier: effect_filter::trust_tier_to_handle_value(entry.abi.effects.trust_tier.as_deref()),
        cost_bound_usd: cost_bound_for(&entry.abi),
        reversible: entry
            .abi
            .effects
            .reversibility
            .as_deref()
            .map(|value| value == "reversible")
            .unwrap_or(false) as u8,
        latency_instant: entry
            .abi
            .effects
            .latency_ms
            .as_ref()
            .map(|latency| latency.p99_estimate <= 1.0)
            .unwrap_or(false) as u8,
        replayable: entry.abi.attributes.replayable as u8,
        deterministic: entry.abi.attributes.deterministic as u8,
        dangerous: entry.abi.attributes.dangerous as u8,
        pub_extern_c: entry.abi.attributes.pub_extern_c as u8,
        requires_approval: entry.abi.approval_contract.required as u8,
        grounded_source_count: entry.abi.provenance.grounded_param_deps.len() as u32,
        param_count: entry.abi.params.len() as u32,
    }
}

fn is_introspection_agent(name: &str) -> bool {
    name.starts_with("__corvid_")
}

fn cost_bound_for(agent: &AbiAgent) -> f64 {
    agent.budget
        .as_ref()
        .map(|budget| budget.usd_per_call)
        .or_else(|| agent.effects.cost.as_ref().map(|cost| cost.projected_usd))
        .unwrap_or(f64::NAN)
}

fn unsupported_message(entry: &AgentCatalogEntry) -> String {
    match &entry.invoker {
        CatalogInvoker::Unsupported { message } => message.clone(),
        _ => "unsupported".to_string(),
    }
}

fn introspection_kind(name: &str) -> Option<IntrospectionKind> {
    match name {
        "__corvid_abi_descriptor_json" => Some(IntrospectionKind::DescriptorJson),
        "__corvid_abi_verify" => Some(IntrospectionKind::Verify),
        "__corvid_list_agents" => Some(IntrospectionKind::ListAgents),
        "__corvid_agent_signature_json" => Some(IntrospectionKind::AgentSignatureJson),
        "__corvid_pre_flight" => Some(IntrospectionKind::PreFlight),
        "__corvid_call_agent" => Some(IntrospectionKind::CallAgent),
        "__corvid_find_agents_where" => Some(IntrospectionKind::FindAgentsWhere),
        _ => None,
    }
}

fn filter_agent_from_entry(
    entry: &AgentCatalogEntry,
    cost_bound_override: Option<f64>,
) -> FilterAgent {
    FilterAgent {
        abi: entry.abi.clone(),
        cost_bound_usd: cost_bound_override.or_else(|| finite_option(cost_bound_for(&entry.abi))),
    }
}

fn finite_option(value: f64) -> Option<f64> {
    if value.is_finite() {
        Some(value)
    } else {
        None
    }
}

pub(crate) fn scalar_param_type_from_descriptor(
    ty: &TypeDescription,
) -> Result<ScalarAbiType, String> {
    match ty {
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Int,
        } => Ok(ScalarAbiType::Int),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Float,
        } => Ok(ScalarAbiType::Float),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Bool,
        } => Ok(ScalarAbiType::Bool),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::String,
        } => Ok(ScalarAbiType::String),
        other => Err(format!(
            "non-scalar parameter type `{other:?}` is deferred to Phase 22-F grounded/structured returns"
        )),
    }
}

pub(crate) fn scalar_return_type_from_descriptor(
    ty: &TypeDescription,
) -> Result<ScalarReturnType, String> {
    match ty {
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Int,
        } => Ok(ScalarReturnType::Int),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Float,
        } => Ok(ScalarReturnType::Float),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Bool,
        } => Ok(ScalarReturnType::Bool),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::String,
        } => Ok(ScalarReturnType::String),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Nothing,
        } => Ok(ScalarReturnType::Nothing),
        TypeDescription::Grounded { grounded } => match grounded.inner.as_ref() {
            TypeDescription::Scalar {
                scalar: ScalarTypeName::Int
                    | ScalarTypeName::Float
                    | ScalarTypeName::Bool
                    | ScalarTypeName::String,
            } => Err(
                "grounded return values are exposed through the direct exported symbol with an attestation handle; generic `corvid_call_agent` JSON dispatch still supports plain scalar returns only"
                    .to_string(),
            ),
            other => Err(format!(
                "grounded return type `{other:?}` is not yet supported by generic host dispatch"
            )),
        },
        other => Err(format!(
            "non-scalar return type `{other:?}` is not supported by generic host dispatch"
        )),
    }
}

fn decode_hex_hash(hex: &str) -> Result<[u8; 32], RuntimeError> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(RuntimeError::Marshal(format!(
            "expected 64 hex chars for SHA-256, got {}",
            hex.len()
        )));
    }
    let mut out = [0u8; 32];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let hi = decode_hex_nibble(chunk[0])?;
        let lo = decode_hex_nibble(chunk[1])?;
        out[index] = (hi << 4) | lo;
    }
    Ok(out)
}

fn decode_hex_nibble(byte: u8) -> Result<u8, RuntimeError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(RuntimeError::Marshal("invalid hex hash".to_string())),
    }
}
