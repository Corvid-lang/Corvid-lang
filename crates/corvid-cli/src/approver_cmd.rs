use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use corvid_driver::{simulate_approver, verify_approver_source};

pub fn run_check(path: &Path, max_budget_usd: Option<f64>) -> Result<u8> {
    verify_approver_source(path, max_budget_usd)
        .map_err(|err| anyhow!("approver check failed for `{}`: {err}", path.display()))?;
    println!("ok");
    Ok(0)
}

pub fn run_simulate(
    path: &Path,
    site_label: &str,
    args_json: &str,
    max_budget_usd: Option<f64>,
) -> Result<u8> {
    if args_json.trim().is_empty() {
        bail!("--args must be a non-empty JSON array");
    }
    let decision = simulate_approver(path, site_label, args_json, max_budget_usd)
        .map_err(|err| anyhow!("approver simulate failed for `{}`: {err}", path.display()))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&decision).context("serialize simulated approval decision")?
    );
    Ok(0)
}
