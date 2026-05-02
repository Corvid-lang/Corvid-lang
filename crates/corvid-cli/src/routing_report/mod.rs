use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

mod build;
pub use build::build_report;

#[derive(Debug, Clone)]
pub struct RoutingReportOptions<'a> {
    pub trace_dir: &'a Path,
    pub since: Option<&'a str>,
    pub since_commit: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutingReport {
    pub trace_dir: String,
    pub since_ms: Option<u64>,
    pub healthy: bool,
    pub model_usage: Vec<ModelUsageRow>,
    pub escalation_patterns: Vec<EscalationRow>,
    pub strategy_rows: Vec<StrategyRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelUsageRow {
    pub prompt: String,
    pub model: String,
    pub call_count: u64,
    pub mean_cost: f64,
    pub mean_confidence: Option<f64>,
    pub p50_latency_ms: Option<u64>,
    pub recommendation: String,
    pub healthy: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EscalationRow {
    pub prompt: String,
    pub stage: usize,
    pub model: Option<String>,
    pub entry_count: u64,
    pub escalation_count: u64,
    pub exhaustion_count: u64,
    pub escalated_pct: f64,
    pub recommendation: String,
    pub healthy: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategyRow {
    pub prompt: String,
    pub strategy: String,
    pub winner_distribution: BTreeMap<String, u64>,
    pub agreement_rate_mean: Option<f64>,
    pub contradiction_rate: Option<f64>,
    pub observed_variant_share: Option<f64>,
    pub declared_rollout_pct: Option<f64>,
    pub recommendation: String,
    pub healthy: bool,
}



pub fn render_report(report: &RoutingReport) -> String {
    let mut out = String::new();
    out.push_str("Model usage\n");
    out.push_str(
        "prompt                model                calls  mean_cost  mean_conf  p50_ms  recommendation\n",
    );
    out.push_str(
        "--------------------- -------------------- ------ ---------- ---------- ------- ---------------------------------------------\n",
    );
    for row in &report.model_usage {
        out.push_str(&format!(
            "{:<21} {:<20} {:>6} {:>10.4} {:>10} {:>7} {}\n",
            row.prompt,
            row.model,
            row.call_count,
            row.mean_cost,
            fmt_conf(row.mean_confidence),
            row.p50_latency_ms
                .map(|ms| ms.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            row.recommendation
        ));
    }

    out.push_str("\nEscalation patterns\n");
    out.push_str(
        "prompt                stage  model                entries  escalations  exhaustions  % escalated  recommendation\n",
    );
    out.push_str(
        "--------------------- ------ -------------------- -------- ------------ ------------ ----------- ---------------------------------------------\n",
    );
    for row in &report.escalation_patterns {
        out.push_str(&format!(
            "{:<21} {:>6} {:<20} {:>8} {:>12} {:>12} {:>11.1} {}\n",
            row.prompt,
            row.stage,
            row.model.clone().unwrap_or_else(|| "n/a".to_string()),
            row.entry_count,
            row.escalation_count,
            row.exhaustion_count,
            row.escalated_pct,
            row.recommendation
        ));
    }

    out.push_str("\nA/B + ensemble + adversarial\n");
    out.push_str(
        "prompt                strategy      winners                          agree_mean  contradiction  rollout%  recommendation\n",
    );
    out.push_str(
        "--------------------- ------------- -------------------------------- ----------- ------------- --------- ---------------------------------------------\n",
    );
    for row in &report.strategy_rows {
        out.push_str(&format!(
            "{:<21} {:<13} {:<32} {:>11} {:>13} {:>9} {}\n",
            row.prompt,
            row.strategy,
            format_distribution(&row.winner_distribution),
            fmt_conf(row.agreement_rate_mean),
            row.contradiction_rate
                .map(|rate| format!("{:.2}", rate))
                .unwrap_or_else(|| "n/a".to_string()),
            row.observed_variant_share
                .map(|pct| format!("{pct:.1}"))
                .unwrap_or_else(|| "n/a".to_string()),
            row.recommendation
        ));
    }
    out
}


pub(super) fn percentile50(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}

pub(super) fn format_distribution(values: &BTreeMap<String, u64>) -> String {
    values
        .iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn fmt_conf(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

pub(super) fn model_label(model: &str, version: Option<&str>) -> String {
    match version {
        Some(version) if !version.is_empty() => format!("{model}@{version}"),
        _ => model.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_report, render_report, RoutingReportOptions};
    use std::path::PathBuf;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/trace-report")
    }

    #[test]
    fn builds_report_from_fixture_dir() {
        let report = build_report(RoutingReportOptions {
            trace_dir: &fixture_dir(),
            since: None,
            since_commit: None,
        })
        .expect("report");
        assert!(!report.model_usage.is_empty());
        assert!(!report.strategy_rows.is_empty());
        let rendered = render_report(&report);
        assert!(rendered.contains("Model usage"));
        assert!(rendered.contains("summarize"));
        assert!(rendered.contains("ensemble"));
    }
}
