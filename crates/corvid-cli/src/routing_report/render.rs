//! Routing-report text formatter — turns the typed
//! `RoutingReport` into the three operator-facing tables (model
//! usage, escalation patterns, A/B + ensemble + adversarial).
//!
//! `format_distribution`, `fmt_conf`, and `model_label` are the
//! per-cell scalar formatters. They're `pub(super)` because
//! `build.rs` reuses `fmt_conf` (to fill the `recommendation`
//! string on a row) and `model_label` (to compute the
//! per-model aggregate key).

use std::collections::BTreeMap;

use super::RoutingReport;

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
