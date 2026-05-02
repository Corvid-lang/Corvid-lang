//! `corvid observe drift` rendering — text formatter for the
//! `LineageDriftReport` (`baseline` vs. `candidate` lineage
//! files) plus the verdict / exit-code policy.
//!
//! `drift_exit_code` reports a regression (returning `1`) when
//! any monitored axis got worse — schema violations, denials,
//! tool errors, cost, or latency increased, or confidence
//! dropped. Stable runs return `0`.

use corvid_runtime::LineageDriftReport;

pub fn render_drift_report(report: &LineageDriftReport) -> String {
    let mut out = String::new();
    out.push_str("corvid observe drift\n");
    out.push_str(&format!(
        "events: baseline={} candidate={} delta={}\n",
        report.baseline.event_count,
        report.candidate.event_count,
        report.candidate.event_count as i128 - report.baseline.event_count as i128
    ));
    out.push_str(&format!(
        "schema_violations: baseline={} candidate={} delta={}\n",
        report.baseline.schema_violation_count,
        report.candidate.schema_violation_count,
        report.schema_violation_delta
    ));
    out.push_str(&format!(
        "cost_usd: baseline={:.6} candidate={:.6} delta={:.6}\n",
        report.baseline.total_cost_usd, report.candidate.total_cost_usd, report.cost_delta_usd
    ));
    out.push_str(&format!(
        "latency_ms: baseline={} candidate={} delta={}\n",
        report.baseline.total_latency_ms,
        report.candidate.total_latency_ms,
        report.latency_delta_ms
    ));
    out.push_str(&format!(
        "denials: baseline={} candidate={} delta={}\n",
        report.baseline.denial_count, report.candidate.denial_count, report.denial_delta
    ));
    out.push_str(&format!(
        "tool_errors: baseline={} candidate={} delta={}\n",
        report.baseline.tool_error_count,
        report.candidate.tool_error_count,
        report.tool_error_delta
    ));
    out.push_str(&format!(
        "confidence: baseline={:.6} candidate={:.6} delta={:.6}\n",
        report.baseline.average_confidence,
        report.candidate.average_confidence,
        report.confidence_delta
    ));
    out.push_str(&format!(
        "verdict: {}\n",
        if drift_exit_code(report) == 0 {
            "stable"
        } else {
            "drift"
        }
    ));
    out
}

pub(super) fn drift_exit_code(report: &LineageDriftReport) -> u8 {
    let regressed = report.schema_violation_delta > 0
        || report.denial_delta > 0
        || report.tool_error_delta > 0
        || report.cost_delta_usd > 0.0
        || report.latency_delta_ms > 0
        || report.confidence_delta < 0.0;
    u8::from(regressed)
}
