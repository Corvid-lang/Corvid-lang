use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

mod build;
mod render;
pub use build::build_report;
pub use render::render_report;

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
