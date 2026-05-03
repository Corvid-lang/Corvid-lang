use super::*;

pub(super) fn compile_approver_source(
    source_path: &Path,
) -> Result<RegisteredApprover, ApproverLoadError> {
    let source = std::fs::read_to_string(source_path).map_err(|err| ApproverLoadError {
        status: CorvidApproverLoadStatus::IoError,
        message: format!("read approver source `{}`: {err}", source_path.display()),
    })?;
    let combined = format!("{APPROVER_PRELUDE}\n{source}");
    let tokens = lex(&combined).map_err(|errs| ApproverLoadError {
        status: CorvidApproverLoadStatus::CompileError,
        message: format!("lex approver source: {errs:?}"),
    })?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::CompileError,
            message: format!("parse approver source: {parse_errors:?}"),
        });
    }
    let resolved = resolve(&file);
    if !resolved.errors.is_empty() {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::CompileError,
            message: format!("resolve approver source: {:?}", resolved.errors),
        });
    }
    let checked = typecheck_with_config(&file, &resolved, None::<&CorvidConfig>);
    if !checked.errors.is_empty() {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::CompileError,
            message: format!("typecheck approver source: {:?}", checked.errors),
        });
    }
    let mut abi_file = file.clone();
    for decl in &mut abi_file.decls {
        if let Decl::Agent(agent) = decl {
            if agent.name.name == APPROVER_AGENT_NAME {
                agent.extern_abi = Some(ExternAbi::C);
            }
        }
    }
    let ir = lower(&abi_file, &resolved, &checked);
    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let registry = EffectRegistry::from_decls(&effect_decls);
    let abi = emit_abi(
        &abi_file,
        &resolved,
        &checked,
        &ir,
        &registry,
        &EmitOptions {
            source_path: &source_path.to_string_lossy().replace('\\', "/"),
            source_text: &combined,
            compiler_version: env!("CARGO_PKG_VERSION"),
            generated_at: "1970-01-01T00:00:00Z",
        },
    )
    .agents
    .into_iter()
    .find(|agent| agent.name == APPROVER_AGENT_NAME)
    .ok_or_else(|| ApproverLoadError {
        status: CorvidApproverLoadStatus::MissingAgent,
        message: format!("no `{APPROVER_AGENT_NAME}` agent found in approver source"),
    })?;
    verify_approver_signature(&abi)?;
    let mut overlay_abi = abi.clone();
    overlay_abi.name = "__corvid_approver".to_string();
    overlay_abi.symbol = "__corvid_approver".to_string();
    overlay_abi.attributes.pub_extern_c = false;
    overlay_abi.attributes.dangerous = false;
    overlay_abi.approval_contract.required = false;
    overlay_abi.approval_contract.labels.clear();
    let signature_json =
        serde_json::to_string_pretty(&overlay_abi).map_err(|err| ApproverLoadError {
            status: CorvidApproverLoadStatus::CompileError,
            message: format!("serialize approver overlay ABI: {err}"),
        })?;
    Ok(RegisteredApprover {
        source_path: source_path.to_path_buf(),
        abi: overlay_abi,
        program: MiniApproverProgram::from_file(&file)?,
        display_budget_usd: f64::NAN,
        signature_json: signature_json.clone(),
        name_c: CString::new("__corvid_approver").expect("valid approver overlay name"),
        symbol_c: CString::new("__corvid_approver").expect("valid approver overlay symbol"),
        source_file_c: CString::new(source_path.to_string_lossy().replace('\\', "/"))
            .expect("valid approver overlay source path"),
        signature_json_c: CString::new(signature_json).expect("valid approver overlay signature"),
    })
}

fn verify_approver_signature(abi: &AbiAgent) -> Result<(), ApproverLoadError> {
    if abi.params.len() != 3 {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::BadSignature,
            message: format!(
                "`{APPROVER_AGENT_NAME}` must take exactly 3 params, got {}",
                abi.params.len()
            ),
        });
    }
    verify_struct(&abi.params[0].ty, "ApprovalSite", "parameter 1 `site`")?;
    verify_struct(&abi.params[1].ty, "ApprovalArgs", "parameter 2 `args`")?;
    verify_struct(&abi.params[2].ty, "ApprovalContext", "parameter 3 `ctx`")?;
    verify_struct(&abi.return_type, "ApprovalDecision", "return type")?;
    Ok(())
}

fn verify_struct(
    ty: &TypeDescription,
    expected: &str,
    where_: &str,
) -> Result<(), ApproverLoadError> {
    match ty {
        TypeDescription::Struct { name } if name == expected => Ok(()),
        other => Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::BadSignature,
            message: format!(
                "`{APPROVER_AGENT_NAME}` {where_} must be `{expected}`, got `{other:?}`"
            ),
        }),
    }
}

pub(super) fn validate_approver_safety(
    abi: &AbiAgent,
    max_budget_usd_per_call: f64,
) -> Result<(), ApproverLoadError> {
    if abi.attributes.dangerous {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::Unsafe,
            message: "approver may not be `@dangerous`".to_string(),
        });
    }
    if abi
        .effects
        .trust_tier
        .as_deref()
        .map(|tier| tier != "autonomous")
        .unwrap_or(false)
    {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::Unsafe,
            message: "approver trust tier must be `autonomous`".to_string(),
        });
    }
    if max_budget_usd_per_call > 0.0 {
        let budget = abi
            .budget
            .as_ref()
            .map(|budget| budget.usd_per_call)
            .or_else(|| abi.effects.cost.as_ref().map(|cost| cost.projected_usd))
            .unwrap_or(0.0);
        if budget > max_budget_usd_per_call {
            return Err(ApproverLoadError {
                status: CorvidApproverLoadStatus::OverBudget,
                message: format!(
                    "approver budget ${budget:.3} exceeds host ceiling ${max_budget_usd_per_call:.3}"
                ),
            });
        }
    }
    Ok(())
}
