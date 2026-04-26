use crate::schema::{AbiToolContract, AbiToolDomainEffect};
use corvid_ast::{DimensionValue, ToolDecl};
use corvid_types::{EffectProfile, EffectRegistry};
use std::collections::BTreeSet;

pub fn emit_tool_contract(tool: &ToolDecl, registry: &EffectRegistry) -> AbiToolContract {
    let mut contract = AbiToolContract::default();
    let mut seen = BTreeSet::new();

    for effect_ref in &tool.effect_row.effects {
        let source_effect = effect_ref.name.name.as_str();
        let Some(profile) = registry.get(source_effect) else {
            continue;
        };
        collect_domain_effects(
            profile,
            source_effect,
            &tool
                .params
                .iter()
                .map(|param| param.name.name.as_str())
                .collect::<Vec<_>>(),
            &mut seen,
            &mut contract,
        );
        if contract.requires_approval.is_none() {
            contract.requires_approval = approval_requirement_from_profile(profile)
                .or_else(|| human_required_profile(profile).then(|| pascal_case(&tool.name.name)));
        }
    }

    if matches!(tool.effect, corvid_ast::Effect::Dangerous) {
        push_domain_effect(&mut contract, &mut seen, "irreversible", None, "dangerous");
        if contract.requires_approval.is_none() {
            contract.requires_approval = Some(pascal_case(&tool.name.name));
        }
    }

    let domain_effects = contract.domain_effects.clone();
    for effect in &domain_effects {
        match effect.kind.as_str() {
            "money" | "irreversible" => push_unique(&mut contract.ci_fail_on, effect.kind.clone()),
            "external" => push_unique(
                &mut contract.approval_card_hints,
                format!(
                    "external system: {}",
                    effect.target.as_deref().unwrap_or("unspecified")
                ),
            ),
            _ => {}
        }
    }
    if let Some(label) = contract.requires_approval.clone() {
        push_unique(
            &mut contract.approval_card_hints,
            format!("requires approval `{label}`"),
        );
    }
    contract
}

fn collect_domain_effects(
    profile: &EffectProfile,
    source_effect: &str,
    param_names: &[&str],
    seen: &mut BTreeSet<String>,
    contract: &mut AbiToolContract,
) {
    let effect_name = profile.name.as_str();
    if effect_name == "money" || effect_name.starts_with("money_") {
        push_domain_effect(
            contract,
            seen,
            "money",
            money_target(profile).or_else(|| first_money_param(param_names)),
            source_effect,
        );
    }
    if effect_name == "external" || effect_name.starts_with("external_") {
        push_domain_effect(
            contract,
            seen,
            "external",
            external_target(profile).or_else(|| suffix_after(effect_name, "external_")),
            source_effect,
        );
    }
    if effect_name == "irreversible" || non_reversible_profile(profile) {
        push_domain_effect(contract, seen, "irreversible", None, source_effect);
    }

    for (name, value) in &profile.dimensions {
        match name.as_str() {
            "domain" if dim_name(value).as_deref() == Some("money") => push_domain_effect(
                contract,
                seen,
                "money",
                money_target(profile).or_else(|| first_money_param(param_names)),
                source_effect,
            ),
            "money" => push_domain_effect(
                contract,
                seen,
                "money",
                dim_value_label(value).or_else(|| first_money_param(param_names)),
                source_effect,
            ),
            "external" => push_domain_effect(
                contract,
                seen,
                "external",
                dim_value_label(value),
                source_effect,
            ),
            "irreversible" => push_domain_effect(
                contract,
                seen,
                "irreversible",
                dim_value_label(value),
                source_effect,
            ),
            _ => {}
        }
    }
}

fn push_domain_effect(
    contract: &mut AbiToolContract,
    seen: &mut BTreeSet<String>,
    kind: &str,
    target: Option<String>,
    source_effect: &str,
) {
    let key = format!("{kind}:{target:?}:{source_effect}");
    if seen.insert(key) {
        contract.domain_effects.push(AbiToolDomainEffect {
            kind: kind.to_string(),
            target,
            source_effect: source_effect.to_string(),
        });
    }
}

fn approval_requirement_from_profile(profile: &EffectProfile) -> Option<String> {
    profile
        .dimensions
        .get("requires_approval")
        .and_then(dim_value_label)
        .map(|label| label.replace('_', "-"))
}

fn human_required_profile(profile: &EffectProfile) -> bool {
    matches!(
        profile.dimensions.get("trust"),
        Some(DimensionValue::Name(tier)) if tier == "human_required"
    )
}

fn non_reversible_profile(profile: &EffectProfile) -> bool {
    matches!(
        profile
            .dimensions
            .get("reversible")
            .or_else(|| profile.dimensions.get("reversibility")),
        Some(DimensionValue::Bool(false))
    ) || matches!(
        profile
            .dimensions
            .get("reversible")
            .or_else(|| profile.dimensions.get("reversibility")),
        Some(DimensionValue::Name(value)) if value == "irreversible" || value == "non_reversible"
    )
}

fn money_target(profile: &EffectProfile) -> Option<String> {
    profile.dimensions.get("money").and_then(dim_value_label)
}

fn external_target(profile: &EffectProfile) -> Option<String> {
    profile.dimensions.get("external").and_then(dim_value_label)
}

fn first_money_param(param_names: &[&str]) -> Option<String> {
    param_names
        .iter()
        .find(|name| matches!(**name, "amount" | "price" | "total" | "usd" | "cents"))
        .map(|name| (*name).to_string())
}

fn suffix_after(value: &str, prefix: &str) -> Option<String> {
    value
        .strip_prefix(prefix)
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
}

fn dim_name(value: &DimensionValue) -> Option<String> {
    match value {
        DimensionValue::Name(value) => Some(value.clone()),
        _ => None,
    }
}

fn dim_value_label(value: &DimensionValue) -> Option<String> {
    match value {
        DimensionValue::Name(value) => Some(value.clone()),
        DimensionValue::Bool(value) => Some(value.to_string()),
        DimensionValue::Cost(value) | DimensionValue::Number(value) => Some(value.to_string()),
        DimensionValue::Streaming { backpressure } => Some(backpressure.label()),
        DimensionValue::ConfidenceGated { threshold, .. } => Some(threshold.to_string()),
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}
