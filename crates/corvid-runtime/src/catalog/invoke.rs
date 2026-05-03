//! Catalog preflight, invocation, and introspection dispatch.

use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use corvid_abi::{AbiAgent, AbiApprovalLabel, ScalarTypeName, TypeDescription};
use corvid_trace_schema::TraceEvent;

use super::{
    agent_signature_json, catalog, descriptor_json, find_agents_where, verify_hash,
    AgentCatalogEntry, CorvidCallStatus, CorvidPreFlightStatus, OwnedApprovalRequired,
    OwnedCallOutcome, OwnedPreFlight, ScalarAbiType, ScalarInvoker, ScalarReturnType,
};

pub(super) enum CatalogInvoker {
    Introspection(IntrospectionKind),
    Scalar(ScalarInvoker),
    Unsupported { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IntrospectionKind {
    DescriptorJson,
    Verify,
    ListAgents,
    AgentSignatureJson,
    PreFlight,
    CallAgent,
    FindAgentsWhere,
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
            observation_handle: crate::observation_handles::NULL_OBSERVATION_HANDLE,
        },
    }
}

fn begin_catalog_run(agent_name: &str, args: &[serde_json::Value]) -> Result<(), RuntimeError> {
    crate::ffi_bridge::corvid_runtime_embed_init_default();
    let runtime = crate::ffi_bridge::runtime();
    runtime.prepare_run(agent_name, args)?;
    let tracer = runtime.tracer();
    if tracer.is_enabled() {
        tracer.emit(TraceEvent::RunStarted {
            ts_ms: now_ms(),
            run_id: tracer.run_id().to_string(),
            agent: agent_name.to_string(),
            args: args.to_vec(),
        });
    }
    Ok(())
}

fn finish_catalog_run(
    ok: bool,
    result: Option<&serde_json::Value>,
    error: Option<&str>,
) -> Result<(), RuntimeError> {
    crate::ffi_bridge::corvid_runtime_embed_init_default();
    let runtime = crate::ffi_bridge::runtime();
    runtime.complete_run(ok, result, error)?;
    let tracer = runtime.tracer();
    if tracer.is_enabled() {
        tracer.emit(TraceEvent::RunCompleted {
            ts_ms: now_ms(),
            run_id: tracer.run_id().to_string(),
            ok,
            result: result.cloned(),
            error: error.map(str::to_string),
        });
    }
    Ok(())
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
    let Some(entry) = state
        .by_name
        .get(agent_name)
        .and_then(|idx| state.agents.get(*idx))
    else {
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
            observation_handle: crate::observation_handles::NULL_OBSERVATION_HANDLE,
        });
    }
    let state = catalog()?;
    let Some(entry) = state
        .by_name
        .get(agent_name)
        .and_then(|idx| state.agents.get(*idx))
    else {
        return Ok(OwnedCallOutcome {
            status: CorvidCallStatus::AgentNotFound,
            result_json: None,
            approval: None,
            observation_handle: crate::observation_handles::NULL_OBSERVATION_HANDLE,
        });
    };
    if let CatalogInvoker::Unsupported { .. } = &entry.invoker {
        return Ok(OwnedCallOutcome {
            status: CorvidCallStatus::UnsupportedSig,
            result_json: None,
            approval: None,
            observation_handle: crate::observation_handles::NULL_OBSERVATION_HANDLE,
        });
    }
    let validated = match validate_args_for_entry(entry, args_json) {
        Ok(validated) => validated,
        Err(err) => {
            return Ok(OwnedCallOutcome {
                status: CorvidCallStatus::BadArgs,
                result_json: Some(serde_json::json!({ "error": err }).to_string()),
                approval: None,
                observation_handle: crate::observation_handles::NULL_OBSERVATION_HANDLE,
            });
        }
    };
    begin_catalog_run(agent_name, &validated.args)?;
    if entry.abi.approval_contract.required {
        let approval = build_approval_required(entry, args_json)?;
        match crate::catalog_c_api::request_host_approval(&approval) {
            crate::catalog_c_api::ApprovalRequestOutcome::MissingOrRejected => {
                emit_embedded_rejected_approval(&approval, &validated.args);
                finish_catalog_run(false, None, Some("approval required"))?;
                return Ok(OwnedCallOutcome {
                    status: CorvidCallStatus::ApprovalRequired,
                    result_json: None,
                    approval: Some(approval),
                    observation_handle: crate::observation_handles::NULL_OBSERVATION_HANDLE,
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

    let invocation = match &entry.invoker {
        CatalogInvoker::Introspection(kind) => {
            let scope = crate::observation_handles::begin_observation(finite_option(
                cost_bound_for(&entry.abi),
            ));
            introspection_call(*kind, validated.args.clone()).map(|result| (result, scope.finish()))
        }
        CatalogInvoker::Scalar(invoker) => (invoker)(&validated.args)
            .map(|invocation| (invocation.result, invocation.observation_handle)),
        CatalogInvoker::Unsupported { .. } => unreachable!(),
    };
    let (result, observation_handle) = match invocation {
        Ok(values) => values,
        Err(err) => {
            let _ = finish_catalog_run(false, None, Some(&err.to_string()));
            return Err(err);
        }
    };
    let result_json = match serde_json::to_string(&result) {
        Ok(result_json) => result_json,
        Err(err) => {
            let err = RuntimeError::Marshal(format!("serialize agent result: {err}"));
            let _ = finish_catalog_run(false, None, Some(&err.to_string()));
            return Err(err);
        }
    };
    finish_catalog_run(true, Some(&result), None)?;
    Ok(OwnedCallOutcome {
        status: CorvidCallStatus::Ok,
        result_json: Some(result_json),
        approval: None,
        observation_handle,
    })
}

struct ValidatedArgs {
    args: Vec<serde_json::Value>,
}

fn validate_args_for_entry(
    entry: &AgentCatalogEntry,
    args_json: &str,
) -> Result<ValidatedArgs, String> {
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

pub(super) fn build_invoker(abi: &AbiAgent) -> Result<CatalogInvoker, RuntimeError> {
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

fn emit_embedded_rejected_approval(approval: &OwnedApprovalRequired, args: &[serde_json::Value]) {
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
        IntrospectionKind::DescriptorJson => {
            Ok(serde_json::Value::String(descriptor_json()?.0.to_string()))
        }
        IntrospectionKind::Verify => {
            let expected = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RuntimeError::Marshal(
                        "`__corvid_abi_verify` expects one String hash argument".to_string(),
                    )
                })?;
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
                agents.push(serde_json::to_value(&overlay.abi).map_err(|err| {
                    RuntimeError::Marshal(format!("serialize approver overlay: {err}"))
                })?);
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
                serde_json::to_string(&agents).map_err(|err| {
                    RuntimeError::Marshal(format!("serialize agent list JSON: {err}"))
                })?,
            ))
        }
        IntrospectionKind::AgentSignatureJson => {
            let name = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RuntimeError::Marshal(
                        "`__corvid_agent_signature_json` expects one String argument".to_string(),
                    )
                })?;
            let signature = agent_signature_json(name)?
                .map(|(json, _, _)| json.to_string())
                .unwrap_or_default();
            Ok(serde_json::Value::String(signature))
        }
        IntrospectionKind::PreFlight => {
            let agent = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RuntimeError::Marshal("`__corvid_pre_flight` expects agent name".to_string())
                })?;
            let args_json = args
                .get(1)
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RuntimeError::Marshal("`__corvid_pre_flight` expects args JSON".to_string())
                })?;
            Ok(serde_json::Value::String(
                serde_json::to_string(&pre_flight(agent, args_json)).map_err(|err| {
                    RuntimeError::Marshal(format!("serialize preflight JSON: {err}"))
                })?,
            ))
        }
        IntrospectionKind::CallAgent => {
            let agent = args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RuntimeError::Marshal("`__corvid_call_agent` expects agent name".to_string())
                })?;
            let args_json = args
                .get(1)
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RuntimeError::Marshal("`__corvid_call_agent` expects args JSON".to_string())
                })?;
            Ok(serde_json::Value::String(
                serde_json::to_string(&call_agent(agent, args_json)).map_err(|err| {
                    RuntimeError::Marshal(format!("serialize call outcome JSON: {err}"))
                })?,
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

pub(super) fn is_introspection_agent(name: &str) -> bool {
    name.starts_with("__corvid_")
}

pub(super) fn cost_bound_for(agent: &AbiAgent) -> f64 {
    agent
        .budget
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

pub(super) fn finite_option(value: f64) -> Option<f64> {
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
