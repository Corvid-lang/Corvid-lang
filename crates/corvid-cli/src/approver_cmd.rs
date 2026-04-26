use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use corvid_driver::{simulate_approver, verify_approver_source};
use corvid_runtime::ApprovalRequest;

use crate::ApproverCardFormat;

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

pub fn run_card(site_label: &str, args_json: &str, format: ApproverCardFormat) -> Result<u8> {
    if args_json.trim().is_empty() {
        bail!("--args must be a non-empty JSON array");
    }
    let args = parse_args(args_json)?;
    let card = ApprovalRequest {
        label: site_label.to_string(),
        args,
    }
    .card();
    match format {
        ApproverCardFormat::Text => print!("{}", card.render_text()),
        ApproverCardFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&card).context("serialize approval card")?
        ),
        ApproverCardFormat::Html => print!("{}", card.render_html()),
    }
    Ok(0)
}

fn parse_args(args_json: &str) -> Result<Vec<serde_json::Value>> {
    match serde_json::from_str::<serde_json::Value>(args_json)
        .with_context(|| format!("--args is not valid JSON: `{args_json}`"))?
    {
        serde_json::Value::Array(args) => Ok(args),
        other => bail!("--args must be a JSON array, got {}", json_kind(&other)),
    }
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
