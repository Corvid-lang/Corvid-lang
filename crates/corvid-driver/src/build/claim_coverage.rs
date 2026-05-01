//! Signed-claim coverage validation.
//!
//! When a Corvid source file declares typed contracts (`@grounded`,
//! `@requires`, `@constrain`, dimensional clauses, etc.) those
//! contracts must each have a counterpart in the ABI descriptor's
//! `signed_claims` array — otherwise the descriptor's signature
//! is making promises the build never enforced. This module walks
//! the AST, collects every guarantee-id implied by the source, and
//! cross-checks against the descriptor before sealing the claim.

use std::collections::BTreeSet;

use corvid_ast::{
    AgentAttribute, Decl, Effect, EffectConstraint, EffectRow, ExtendMethodKind, File, PromptDecl,
    ToolDecl, TypeRef,
};
use corvid_guarantees::{lookup as lookup_guarantee, GuaranteeClass};

pub fn validate_signed_claim_coverage(file: &File, descriptor_json: &str) -> anyhow::Result<()> {
    let descriptor = corvid_abi::descriptor_from_json(descriptor_json)
        .map_err(|err| anyhow::anyhow!("signed claim descriptor JSON is malformed: {err}"))?;
    let claim_ids = descriptor
        .claim_guarantees
        .iter()
        .map(|guarantee| guarantee.id.as_str())
        .collect::<BTreeSet<_>>();
    if claim_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "`corvid build --sign` refused: ABI descriptor has no signed claim guarantee set"
        ));
    }

    let mut failures = Vec::new();
    for guarantee in &descriptor.claim_guarantees {
        match lookup_guarantee(&guarantee.id) {
            Some(registered) if registered.class != GuaranteeClass::OutOfScope => {}
            Some(_) => failures.push(format!(
                "descriptor claim `{}` is registered as out_of_scope",
                guarantee.id
            )),
            None => failures.push(format!(
                "descriptor claim `{}` is not registered in GUARANTEE_REGISTRY",
                guarantee.id
            )),
        }
    }

    let source_claims = declared_contract_claims(file);
    for id in &source_claims.required_ids {
        if !claim_ids.contains(id) {
            failures.push(format!(
                "source-declared contract requires `{id}`, but the signed descriptor claim set does not include it"
            ));
        }
    }
    failures.extend(source_claims.unsupported);

    if !failures.is_empty() {
        return Err(anyhow::anyhow!(
            "`corvid build --sign` refused because the signed ABI claim would be incomplete:\n  - {}",
            failures.join("\n  - ")
        ));
    }
    Ok(())
}

#[derive(Default)]
struct DeclaredContractClaims {
    required_ids: BTreeSet<&'static str>,
    unsupported: Vec<String>,
}

fn declared_contract_claims(file: &File) -> DeclaredContractClaims {
    let mut claims = DeclaredContractClaims::default();
    claims.required_ids.extend([
        "abi_descriptor.cdylib_emission",
        "abi_descriptor.byte_determinism",
        "abi_descriptor.bilateral_source_match",
        "abi_attestation.envelope_signature",
        "abi_attestation.descriptor_match",
        "abi_attestation.sign_requires_claim_coverage",
    ]);
    for decl in &file.decls {
        collect_decl_contracts(decl, &mut claims);
    }
    claims
}

fn collect_decl_contracts(decl: &Decl, claims: &mut DeclaredContractClaims) {
    match decl {
        Decl::Import(import) => {
            if !import.effect_row.is_empty()
                || !import.required_attributes.is_empty()
                || !import.required_constraints.is_empty()
            {
                claims.required_ids.insert("effect_row.import_boundary");
            }
        }
        Decl::Tool(tool) => collect_tool_contracts(tool, claims),
        Decl::Prompt(prompt) => collect_prompt_contracts(prompt, claims),
        Decl::Agent(agent) => {
            collect_effect_row_claims(&agent.effect_row, claims);
            collect_type_claims(&agent.return_ty, claims);
            for param in &agent.params {
                collect_type_claims(&param.ty, claims);
            }
            for constraint in &agent.constraints {
                collect_constraint_claims(&agent.name.name, constraint, claims);
            }
            for attr in &agent.attributes {
                match attr {
                    AgentAttribute::Replayable { .. } | AgentAttribute::Deterministic { .. } => {
                        claims.required_ids.insert("replay.deterministic_pure_path");
                    }
                    AgentAttribute::Wrapping { .. } => claims.unsupported.push(format!(
                        "agent `{}` declares `@wrapping`, but no signed cdylib guarantee id covers wrapping arithmetic yet",
                        agent.name.name
                    )),
                }
            }
        }
        Decl::Extend(extend) => {
            for method in &extend.methods {
                match &method.kind {
                    ExtendMethodKind::Tool(tool) => collect_tool_contracts(tool, claims),
                    ExtendMethodKind::Prompt(prompt) => collect_prompt_contracts(prompt, claims),
                    ExtendMethodKind::Agent(agent) => {
                        collect_decl_contracts(&Decl::Agent(agent.clone()), claims)
                    }
                }
            }
        }
        Decl::Eval(eval) => {
            for assertion in &eval.assertions {
                match assertion {
                    corvid_ast::EvalAssert::Value { confidence, .. } => {
                        if confidence.is_some() {
                            claims.required_ids.insert("confidence.min_threshold");
                        }
                    }
                    corvid_ast::EvalAssert::Approved { .. } => {
                        claims.required_ids.insert("approval.dangerous_call_requires_token");
                        claims.required_ids.insert("approval.token_lexical_only");
                    }
                    corvid_ast::EvalAssert::Cost { .. } => {
                        claims.required_ids.insert("budget.compile_time_ceiling");
                    }
                    corvid_ast::EvalAssert::Snapshot { .. }
                    | corvid_ast::EvalAssert::Called { .. }
                    | corvid_ast::EvalAssert::Ordering { .. } => {}
                }
            }
        }
        Decl::Schedule(schedule) => {
            // A `schedule "cron" zone "…" -> job(args)` declaration
            // implies a durable cron trigger (Phase 38). Slice 35-N
            // requires the cdylib's signed claim set to acknowledge
            // jobs.cron_schedule_durable so a binary that ships a
            // cron trigger cannot omit the corresponding guarantee.
            claims.required_ids.insert("jobs.cron_schedule_durable");
            collect_effect_row_claims(&schedule.effect_row, claims);
        }
        _ => {}
    }
}

fn collect_tool_contracts(tool: &ToolDecl, claims: &mut DeclaredContractClaims) {
    if tool.effect == Effect::Dangerous {
        claims.required_ids.insert("approval.dangerous_call_requires_token");
        claims.required_ids.insert("approval.token_lexical_only");
        claims.required_ids.insert("approval.dangerous_marker_preserved");
    }
    collect_effect_row_claims(&tool.effect_row, claims);
    collect_type_claims(&tool.return_ty, claims);
    for param in &tool.params {
        collect_type_claims(&param.ty, claims);
    }
}

fn collect_prompt_contracts(prompt: &PromptDecl, claims: &mut DeclaredContractClaims) {
    collect_effect_row_claims(&prompt.effect_row, claims);
    collect_type_claims(&prompt.return_ty, claims);
    for param in &prompt.params {
        collect_type_claims(&param.ty, claims);
    }
    if prompt.cites_strictly.is_some() {
        claims.required_ids.insert("grounded.provenance_required");
        claims.required_ids.insert("grounded.propagation_across_calls");
    }
    if prompt.stream.min_confidence.is_some() {
        claims.required_ids.insert("confidence.min_threshold");
    }
    if prompt.calibrated {
        claims.unsupported.push(format!(
            "prompt `{}` declares `calibrated`, but no signed cdylib guarantee id covers calibration yet",
            prompt.name.name
        ));
    }
    if prompt.cacheable {
        claims.unsupported.push(format!(
            "prompt `{}` declares `cacheable`, but no signed cdylib guarantee id covers prompt cache purity yet",
            prompt.name.name
        ));
    }
    if prompt.capability_required.is_some() {
        claims.unsupported.push(format!(
            "prompt `{}` declares `requires`, but no signed cdylib guarantee id covers model capability routing yet",
            prompt.name.name
        ));
    }
    if prompt.output_format_required.is_some() {
        claims.unsupported.push(format!(
            "prompt `{}` declares `output_format`, but no signed cdylib guarantee id covers output-format enforcement yet",
            prompt.name.name
        ));
    }
    if prompt.route.is_some()
        || prompt.progressive.is_some()
        || prompt.rollout.is_some()
        || prompt.ensemble.is_some()
        || prompt.adversarial.is_some()
    {
        claims.unsupported.push(format!(
            "prompt `{}` declares model dispatch policy, but no signed cdylib guarantee id covers dispatch correctness yet",
            prompt.name.name
        ));
    }
}

fn collect_effect_row_claims(row: &EffectRow, claims: &mut DeclaredContractClaims) {
    if !row.is_empty() {
        claims.required_ids.insert("effect_row.body_completeness");
        claims.required_ids.insert("effect_row.caller_propagation");
    }
}

fn collect_constraint_claims(
    agent_name: &str,
    constraint: &EffectConstraint,
    claims: &mut DeclaredContractClaims,
) {
    match constraint.dimension.name.as_str() {
        "budget" | "cost" => {
            claims.required_ids.insert("budget.compile_time_ceiling");
        }
        "min_confidence" | "confidence" => {
            claims.required_ids.insert("confidence.min_threshold");
        }
        other => claims.unsupported.push(format!(
            "agent `{agent_name}` declares `@{other}(...)`, but no signed cdylib guarantee id covers that effect constraint yet"
        )),
    }
}

fn collect_type_claims(ty: &TypeRef, claims: &mut DeclaredContractClaims) {
    if type_ref_contains_grounded(ty) {
        claims.required_ids.insert("grounded.provenance_required");
        claims.required_ids.insert("grounded.propagation_across_calls");
    }
}

fn type_ref_contains_grounded(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Named { name, .. } => name.name == "Grounded",
        TypeRef::Qualified { .. } => false,
        TypeRef::Generic { name, args, .. } => {
            name.name == "Grounded" || args.iter().any(type_ref_contains_grounded)
        }
        TypeRef::Weak { inner, .. } => type_ref_contains_grounded(inner),
        TypeRef::Function { params, ret, .. } => {
            params.iter().any(type_ref_contains_grounded) || type_ref_contains_grounded(ret)
        }
    }
}
