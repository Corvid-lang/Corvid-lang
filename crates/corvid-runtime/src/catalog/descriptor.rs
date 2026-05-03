//! Embedded catalog descriptor loading and descriptor-facing accessors.

use crate::effect_filter;
use crate::errors::RuntimeError;
use corvid_abi::{descriptor_from_embedded_section, AbiAgent, AbiApprovalSite};
use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::sync::OnceLock;

use super::{
    build_invoker, cost_bound_for, is_introspection_agent, CatalogInvoker, CorvidAgentHandle,
    CorvidTrustTier,
};

pub(super) struct AgentCatalogEntry {
    pub(super) abi: AbiAgent,
    pub(super) name_c: CString,
    pub(super) symbol_c: CString,
    pub(super) source_file_c: CString,
    pub(super) signature_json_c: CString,
    pub(super) effect_row_json: String,
    pub(super) grounded_source_set_json: String,
    pub(super) invoker: CatalogInvoker,
}

pub(super) struct CatalogState {
    pub(super) descriptor_json: String,
    pub(super) descriptor_json_c: CString,
    pub(super) descriptor_hash: [u8; 32],
    pub(super) approval_sites: Vec<AbiApprovalSite>,
    pub(super) agents: Vec<AgentCatalogEntry>,
    pub(super) by_name: HashMap<String, usize>,
}

struct CatalogInit(Result<CatalogState, RuntimeError>);

static CATALOG: OnceLock<CatalogInit> = OnceLock::new();

pub fn descriptor_json() -> Result<(&'static str, usize), RuntimeError> {
    let state = catalog()?;
    Ok((&state.descriptor_json, state.descriptor_json.len()))
}

pub fn descriptor_json_ptr() -> Result<(*const c_char, usize), RuntimeError> {
    let state = catalog()?;
    Ok((
        state.descriptor_json_c.as_ptr(),
        state.descriptor_json.len(),
    ))
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
    for entry in state
        .agents
        .iter()
        .filter(|entry| is_introspection_agent(&entry.abi.name))
    {
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
        return Ok(
            crate::approver_bridge::registered_approver_overlay().map(|overlay| {
                let value: &'static str = Box::leak(overlay.signature_json.into_boxed_str());
                (
                    value,
                    overlay.signature_json_len,
                    overlay.signature_json_ptr,
                )
            }),
        );
    }
    let state = catalog()?;
    let Some(entry) = state
        .by_name
        .get(name)
        .and_then(|idx| state.agents.get(*idx))
    else {
        return Ok(None);
    };
    let value = entry
        .signature_json_c
        .to_str()
        .map_err(|err| RuntimeError::Other(format!("catalog signature UTF-8 bug: {err}")))?;
    Ok(Some((value, value.len(), entry.signature_json_c.as_ptr())))
}

pub(super) fn catalog() -> Result<&'static CatalogState, RuntimeError> {
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
            .map_err(|err| {
            RuntimeError::Other(format!("serialize grounded source set: {err}"))
        })?;
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

fn cstring(value: &str) -> Result<CString, RuntimeError> {
    CString::new(value)
        .map_err(|err| RuntimeError::Other(format!("catalog string contained NUL: {err}")))
}

fn handle_from_entry(entry: &AgentCatalogEntry) -> CorvidAgentHandle {
    CorvidAgentHandle {
        name: entry.name_c.as_ptr(),
        symbol: entry.symbol_c.as_ptr(),
        source_file: entry.source_file_c.as_ptr(),
        source_line: entry.abi.source_line,
        trust_tier: effect_filter::trust_tier_to_handle_value(
            entry.abi.effects.trust_tier.as_deref(),
        ),
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
