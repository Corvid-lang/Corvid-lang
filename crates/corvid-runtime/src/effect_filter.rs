use crate::catalog::CorvidTrustTier;
use corvid_abi::AbiAgent;
use std::collections::BTreeSet;

const TRUST_TIER_ORDER: &[&str] = &["autonomous", "supervisor_required", "human_required"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[repr(C)]
pub enum CorvidFindAgentsStatus {
    Ok = 0,
    BadJson = 1,
    UnknownDimension = 2,
    OpMismatch = 3,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OwnedFindAgentsResult {
    pub status: CorvidFindAgentsStatus,
    pub matched_indices: Vec<usize>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct FilterAgent {
    pub abi: AbiAgent,
    pub cost_bound_usd: Option<f64>,
}

#[derive(Debug, Clone)]
enum FilterExpr {
    All(Vec<FilterExpr>),
    Any(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
    Leaf(LeafPredicate),
}

#[derive(Debug, Clone)]
struct LeafPredicate {
    dim: String,
    op: FilterOp,
    value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchState {
    Match,
    NoMatch,
    Unspecified,
}

#[derive(Debug, Clone)]
struct FilterError {
    status: CorvidFindAgentsStatus,
    message: String,
}

impl FilterError {
    fn bad_json(message: impl Into<String>) -> Self {
        Self {
            status: CorvidFindAgentsStatus::BadJson,
            message: message.into(),
        }
    }

    fn unknown_dimension(message: impl Into<String>) -> Self {
        Self {
            status: CorvidFindAgentsStatus::UnknownDimension,
            message: message.into(),
        }
    }

    fn op_mismatch(message: impl Into<String>) -> Self {
        Self {
            status: CorvidFindAgentsStatus::OpMismatch,
            message: message.into(),
        }
    }
}

pub(crate) fn find_matching_indices(
    agents: &[FilterAgent],
    filter_json: &str,
) -> OwnedFindAgentsResult {
    match find_matching_indices_impl(agents, filter_json) {
        Ok(matched_indices) => OwnedFindAgentsResult {
            status: CorvidFindAgentsStatus::Ok,
            matched_indices,
            error_message: None,
        },
        Err(err) => OwnedFindAgentsResult {
            status: err.status,
            matched_indices: Vec::new(),
            error_message: Some(err.message),
        },
    }
}

fn find_matching_indices_impl(
    agents: &[FilterAgent],
    filter_json: &str,
) -> Result<Vec<usize>, FilterError> {
    let value: serde_json::Value = serde_json::from_str(filter_json)
        .map_err(|err| FilterError::bad_json(format!("filter JSON parse failed: {err}")))?;
    let expr = parse_filter_expr(&value)?;
    let known_custom_dims = agents
        .iter()
        .flat_map(|agent| agent.abi.effects.custom.keys().cloned())
        .collect::<BTreeSet<_>>();
    let mut out = Vec::new();
    for (index, agent) in agents.iter().enumerate() {
        if matches!(evaluate_expr(&expr, agent, &known_custom_dims)?, MatchState::Match) {
            out.push(index);
        }
    }
    Ok(out)
}

fn parse_filter_expr(value: &serde_json::Value) -> Result<FilterExpr, FilterError> {
    let serde_json::Value::Object(map) = value else {
        return Err(FilterError::bad_json(
            "filter expression must be a JSON object",
        ));
    };

    if let Some(value) = map.get("all") {
        if map.len() != 1 {
            return Err(FilterError::bad_json(
                "`all` expression may not carry sibling keys",
            ));
        }
        return Ok(FilterExpr::All(parse_expr_list("all", value)?));
    }
    if let Some(value) = map.get("any") {
        if map.len() != 1 {
            return Err(FilterError::bad_json(
                "`any` expression may not carry sibling keys",
            ));
        }
        return Ok(FilterExpr::Any(parse_expr_list("any", value)?));
    }
    if let Some(value) = map.get("not") {
        if map.len() != 1 {
            return Err(FilterError::bad_json(
                "`not` expression may not carry sibling keys",
            ));
        }
        return Ok(FilterExpr::Not(Box::new(parse_filter_expr(value)?)));
    }

    let dim = map
        .get("dim")
        .and_then(|value| value.as_str())
        .ok_or_else(|| FilterError::bad_json("leaf predicate must include string field `dim`"))?;
    let op = map
        .get("op")
        .and_then(|value| value.as_str())
        .ok_or_else(|| FilterError::bad_json("leaf predicate must include string field `op`"))?;
    let value = map
        .get("value")
        .cloned()
        .ok_or_else(|| FilterError::bad_json("leaf predicate must include field `value`"))?;
    if map.len() != 3 {
        return Err(FilterError::bad_json(
            "leaf predicate may only contain `dim`, `op`, and `value`",
        ));
    }
    Ok(FilterExpr::Leaf(LeafPredicate {
        dim: dim.to_string(),
        op: parse_op(op)?,
        value,
    }))
}

fn parse_expr_list(kind: &str, value: &serde_json::Value) -> Result<Vec<FilterExpr>, FilterError> {
    let serde_json::Value::Array(items) = value else {
        return Err(FilterError::bad_json(format!(
            "`{kind}` expects a JSON array of expressions"
        )));
    };
    if items.is_empty() {
        return Err(FilterError::bad_json(format!(
            "`{kind}` expects at least one child expression"
        )));
    }
    items.iter().map(parse_filter_expr).collect()
}

fn parse_op(raw: &str) -> Result<FilterOp, FilterError> {
    match raw {
        "eq" => Ok(FilterOp::Eq),
        "ne" => Ok(FilterOp::Ne),
        "lt" => Ok(FilterOp::Lt),
        "le" => Ok(FilterOp::Le),
        "gt" => Ok(FilterOp::Gt),
        "ge" => Ok(FilterOp::Ge),
        _ => Err(FilterError::bad_json(format!(
            "unsupported operator `{raw}`; expected one of eq/ne/lt/le/gt/ge"
        ))),
    }
}

fn evaluate_expr(
    expr: &FilterExpr,
    agent: &FilterAgent,
    known_custom_dims: &BTreeSet<String>,
) -> Result<MatchState, FilterError> {
    match expr {
        FilterExpr::All(children) => {
            let mut saw_unspecified = false;
            for child in children {
                match evaluate_expr(child, agent, known_custom_dims)? {
                    MatchState::Match => {}
                    MatchState::NoMatch => return Ok(MatchState::NoMatch),
                    MatchState::Unspecified => saw_unspecified = true,
                }
            }
            Ok(if saw_unspecified {
                MatchState::Unspecified
            } else {
                MatchState::Match
            })
        }
        FilterExpr::Any(children) => {
            let mut saw_unspecified = false;
            for child in children {
                match evaluate_expr(child, agent, known_custom_dims)? {
                    MatchState::Match => return Ok(MatchState::Match),
                    MatchState::NoMatch => {}
                    MatchState::Unspecified => saw_unspecified = true,
                }
            }
            Ok(if saw_unspecified {
                MatchState::Unspecified
            } else {
                MatchState::NoMatch
            })
        }
        FilterExpr::Not(child) => Ok(match evaluate_expr(child, agent, known_custom_dims)? {
            MatchState::Match => MatchState::NoMatch,
            MatchState::NoMatch => MatchState::Match,
            MatchState::Unspecified => MatchState::Unspecified,
        }),
        FilterExpr::Leaf(leaf) => evaluate_leaf(leaf, agent, known_custom_dims),
    }
}

fn evaluate_leaf(
    leaf: &LeafPredicate,
    agent: &FilterAgent,
    known_custom_dims: &BTreeSet<String>,
) -> Result<MatchState, FilterError> {
    match leaf.dim.as_str() {
        "trust_tier" => match agent.abi.effects.trust_tier.as_deref() {
            Some(actual) => compare_tier(&leaf.op, actual, &leaf.value),
            None => Ok(MatchState::Unspecified),
        },
        "cost_bound_usd" => match agent.cost_bound_usd {
            Some(actual) => compare_number(&leaf.op, actual, &leaf.value, "cost_bound_usd"),
            None => Ok(MatchState::Unspecified),
        },
        "latency_p99_ms" => match agent.abi.effects.latency_ms.as_ref() {
            Some(latency) => compare_number(
                &leaf.op,
                latency.p99_estimate,
                &leaf.value,
                "latency_p99_ms",
            ),
            None => Ok(MatchState::Unspecified),
        },
        "dangerous" => compare_bool(&leaf.op, agent.abi.attributes.dangerous, &leaf.value, "dangerous"),
        "replayable" => compare_bool(
            &leaf.op,
            agent.abi.attributes.replayable,
            &leaf.value,
            "replayable",
        ),
        "deterministic" => compare_bool(
            &leaf.op,
            agent.abi.attributes.deterministic,
            &leaf.value,
            "deterministic",
        ),
        "reversible" => match agent.abi.effects.reversibility.as_deref() {
            Some("reversible") => compare_bool(&leaf.op, true, &leaf.value, "reversible"),
            Some("non_reversible") | Some("irreversible") => {
                compare_bool(&leaf.op, false, &leaf.value, "reversible")
            }
            Some(other) => Err(FilterError::op_mismatch(format!(
                "dimension `reversible` saw unsupported reversibility value `{other}`"
            ))),
            None => Ok(MatchState::Unspecified),
        },
        other if known_custom_dims.contains(other) => match agent.abi.effects.custom.get(other) {
            Some(value) => compare_custom_value(other, &leaf.op, value, &leaf.value),
            None => Ok(MatchState::Unspecified),
        },
        other => Err(FilterError::unknown_dimension(format!(
            "unknown effect dimension `{other}`"
        ))),
    }
}

fn compare_tier(
    op: &FilterOp,
    actual: &str,
    expected: &serde_json::Value,
) -> Result<MatchState, FilterError> {
    let Some(expected) = expected.as_str() else {
        return Err(FilterError::op_mismatch(
            "dimension `trust_tier` expects a string value",
        ));
    };
    let actual_rank = trust_tier_rank(actual).ok_or_else(|| {
        FilterError::op_mismatch(format!(
            "dimension `trust_tier` saw unknown tier `{actual}`"
        ))
    })?;
    let expected_rank = trust_tier_rank(expected).ok_or_else(|| {
        FilterError::op_mismatch(format!(
            "dimension `trust_tier` does not recognize `{expected}`"
        ))
    })?;
    Ok(if compare_ordinals(*op, actual_rank, expected_rank) {
        MatchState::Match
    } else {
        MatchState::NoMatch
    })
}

fn compare_number(
    op: &FilterOp,
    actual: f64,
    expected: &serde_json::Value,
    dim: &str,
) -> Result<MatchState, FilterError> {
    let Some(expected) = expected.as_f64() else {
        return Err(FilterError::op_mismatch(format!(
            "dimension `{dim}` expects a numeric value"
        )));
    };
    Ok(if compare_f64(*op, actual, expected) {
        MatchState::Match
    } else {
        MatchState::NoMatch
    })
}

fn compare_bool(
    op: &FilterOp,
    actual: bool,
    expected: &serde_json::Value,
    dim: &str,
) -> Result<MatchState, FilterError> {
    let Some(expected) = expected.as_bool() else {
        return Err(FilterError::op_mismatch(format!(
            "dimension `{dim}` expects a boolean value"
        )));
    };
    match op {
        FilterOp::Eq => Ok(if actual == expected {
            MatchState::Match
        } else {
            MatchState::NoMatch
        }),
        FilterOp::Ne => Ok(if actual != expected {
            MatchState::Match
        } else {
            MatchState::NoMatch
        }),
        _ => Err(FilterError::op_mismatch(format!(
            "dimension `{dim}` only supports `eq` and `ne`"
        ))),
    }
}

fn compare_custom_value(
    dim: &str,
    op: &FilterOp,
    actual: &serde_json::Value,
    expected: &serde_json::Value,
) -> Result<MatchState, FilterError> {
    match (actual, expected) {
        (serde_json::Value::Bool(actual), serde_json::Value::Bool(expected)) => match op {
            FilterOp::Eq => Ok(if actual == expected {
                MatchState::Match
            } else {
                MatchState::NoMatch
            }),
            FilterOp::Ne => Ok(if actual != expected {
                MatchState::Match
            } else {
                MatchState::NoMatch
            }),
            _ => Err(FilterError::op_mismatch(format!(
                "custom dimension `{dim}` only supports `eq` and `ne` for boolean values"
            ))),
        },
        (serde_json::Value::Number(actual), serde_json::Value::Number(expected)) => compare_number(
            op,
            actual
                .as_f64()
                .ok_or_else(|| FilterError::op_mismatch(format!(
                    "dimension `{dim}` contained non-finite numeric value"
                )))?,
            &serde_json::Value::Number(expected.clone()),
            dim,
        ),
        (serde_json::Value::String(actual), serde_json::Value::String(expected)) => {
            match op {
                FilterOp::Eq => Ok(if actual == expected {
                    MatchState::Match
                } else {
                    MatchState::NoMatch
                }),
                FilterOp::Ne => Ok(if actual != expected {
                    MatchState::Match
                } else {
                    MatchState::NoMatch
                }),
                _ => Err(FilterError::op_mismatch(format!(
                    "custom dimension `{dim}` only supports ordered comparisons for numeric values"
                ))),
            }
        }
        _ => Err(FilterError::op_mismatch(format!(
            "custom dimension `{dim}` value type mismatch between descriptor and filter"
        ))),
    }
}

fn compare_ordinals(op: FilterOp, actual: u8, expected: u8) -> bool {
    match op {
        FilterOp::Eq => actual == expected,
        FilterOp::Ne => actual != expected,
        FilterOp::Lt => actual < expected,
        FilterOp::Le => actual <= expected,
        FilterOp::Gt => actual > expected,
        FilterOp::Ge => actual >= expected,
    }
}

fn compare_f64(op: FilterOp, actual: f64, expected: f64) -> bool {
    match op {
        FilterOp::Eq => actual == expected,
        FilterOp::Ne => actual != expected,
        FilterOp::Lt => actual < expected,
        FilterOp::Le => actual <= expected,
        FilterOp::Gt => actual > expected,
        FilterOp::Ge => actual >= expected,
    }
}

fn trust_tier_rank(value: &str) -> Option<u8> {
    TRUST_TIER_ORDER
        .iter()
        .position(|candidate| *candidate == value)
        .map(|index| index as u8)
}

pub(crate) fn trust_tier_to_handle_value(value: Option<&str>) -> u8 {
    match value {
        Some("autonomous") => CorvidTrustTier::Autonomous as u8,
        Some("human_required") => CorvidTrustTier::HumanRequired as u8,
        Some("security_review") => CorvidTrustTier::SecurityReview as u8,
        Some(_) => 3,
        None => CorvidTrustTier::Autonomous as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_abi::{
        AbiApprovalContract, AbiAttributes, AbiEffects, AbiProvenanceContract, AbiSourceSpan,
        ScalarTypeName, TypeDescription,
    };

    fn scalar_string() -> TypeDescription {
        TypeDescription::Scalar {
            scalar: ScalarTypeName::String,
        }
    }

    fn agent(name: &str) -> FilterAgent {
        FilterAgent {
            abi: AbiAgent {
                name: name.to_string(),
                symbol: name.to_string(),
                source_span: AbiSourceSpan { start: 0, end: 0 },
                source_line: 1,
                params: Vec::new(),
                return_type: scalar_string(),
                effects: AbiEffects::default(),
                attributes: AbiAttributes::default(),
                budget: None,
                required_capability: None,
                dispatch: None,
                approval_contract: AbiApprovalContract {
                    required: false,
                    labels: Vec::new(),
                },
                provenance: AbiProvenanceContract {
                    returns_grounded: false,
                    grounded_param_deps: Vec::new(),
                },
            },
            cost_bound_usd: None,
        }
    }

    #[test]
    fn unspecified_ordered_field_is_omitted_from_matches() {
        let mut with_trust = agent("with_trust");
        with_trust.abi.effects.trust_tier = Some("autonomous".into());
        let missing_trust = agent("missing_trust");
        let result = find_matching_indices(
            &[with_trust, missing_trust],
            r#"{"dim":"trust_tier","op":"le","value":"autonomous"}"#,
        );
        assert_eq!(result.status, CorvidFindAgentsStatus::Ok);
        assert_eq!(result.matched_indices, vec![0]);
    }

    #[test]
    fn all_any_and_not_compose_with_tristate_semantics() {
        let mut a = agent("a");
        a.abi.effects.trust_tier = Some("autonomous".into());
        a.cost_bound_usd = Some(0.05);
        let mut b = agent("b");
        b.abi.effects.trust_tier = Some("human_required".into());
        b.cost_bound_usd = Some(0.05);
        let c = agent("c");
        let result = find_matching_indices(
            &[a, b, c],
            r#"{"all":[{"dim":"cost_bound_usd","op":"le","value":0.10},{"not":{"dim":"trust_tier","op":"gt","value":"autonomous"}}]}"#,
        );
        assert_eq!(result.status, CorvidFindAgentsStatus::Ok);
        assert_eq!(result.matched_indices, vec![0]);
    }

    #[test]
    fn unknown_dimension_is_typed_error() {
        let result = find_matching_indices(
            &[agent("a")],
            r#"{"dim":"made_up","op":"eq","value":true}"#,
        );
        assert_eq!(result.status, CorvidFindAgentsStatus::UnknownDimension);
        assert!(result
            .error_message
            .unwrap()
            .contains("unknown effect dimension"));
    }

    #[test]
    fn op_mismatch_is_typed_error_for_bool_dimension() {
        let mut dangerous = agent("dangerous");
        dangerous.abi.attributes.dangerous = true;
        let result = find_matching_indices(
            &[dangerous],
            r#"{"dim":"dangerous","op":"le","value":true}"#,
        );
        assert_eq!(result.status, CorvidFindAgentsStatus::OpMismatch);
        assert!(result.error_message.unwrap().contains("only supports `eq` and `ne`"));
    }

    #[test]
    fn custom_dimensions_filter_when_declared() {
        let mut a = agent("a");
        a.abi.effects.custom.insert(
            "risk_score".into(),
            serde_json::Value::Number(serde_json::Number::from_f64(0.2).unwrap()),
        );
        let mut b = agent("b");
        b.abi.effects.custom.insert(
            "risk_score".into(),
            serde_json::Value::Number(serde_json::Number::from_f64(0.8).unwrap()),
        );
        let result = find_matching_indices(
            &[a, b],
            r#"{"dim":"risk_score","op":"le","value":0.5}"#,
        );
        assert_eq!(result.status, CorvidFindAgentsStatus::Ok);
        assert_eq!(result.matched_indices, vec![0]);
    }

    #[test]
    fn trust_tier_order_matches_checker_builtin_lattice() {
        assert_eq!(
            TRUST_TIER_ORDER,
            corvid_types::effects::BUILTIN_TRUST_TIERS
        );
    }
}
