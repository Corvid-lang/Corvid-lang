use std::path::Path;

use corvid_runtime::approver_bridge::{
    clear_registered_approver, register_approver_from_source, simulate_approver_source,
    SimulatedApproverDecision,
};

pub fn verify_approver_source(path: &Path, max_budget_usd_per_call: Option<f64>) -> Result<(), String> {
    register_approver_from_source(path, max_budget_usd_per_call.unwrap_or(0.0))
        .map_err(|err| err.message)?;
    clear_registered_approver();
    Ok(())
}

pub fn simulate_approver(
    path: &Path,
    site_label: &str,
    args_json: &str,
    max_budget_usd_per_call: Option<f64>,
) -> Result<SimulatedApproverDecision, String> {
    simulate_approver_source(
        path,
        site_label,
        args_json,
        max_budget_usd_per_call.unwrap_or(0.0),
    )
    .map_err(|err| err.message)
}
