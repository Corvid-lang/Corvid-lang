//! Catalog agent filtering queries.

use crate::effect_filter::{self, CorvidFindAgentsStatus, FilterAgent};
use crate::errors::RuntimeError;

use super::{
    catalog, cost_bound_for, finite_option, is_introspection_agent, AgentCatalogEntry,
    OwnedFindAgentsOutcome,
};

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

fn find_agents_where_impl(filter_json: &str) -> Result<OwnedFindAgentsOutcome, RuntimeError> {
    let state = catalog()?;
    let mut agents = Vec::with_capacity(state.agents.len() + 1);
    for entry in state
        .agents
        .iter()
        .filter(|entry| is_introspection_agent(&entry.abi.name))
    {
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

fn filter_agent_from_entry(
    entry: &AgentCatalogEntry,
    cost_bound_override: Option<f64>,
) -> FilterAgent {
    FilterAgent {
        abi: entry.abi.clone(),
        cost_bound_usd: cost_bound_override.or_else(|| finite_option(cost_bound_for(&entry.abi))),
    }
}
