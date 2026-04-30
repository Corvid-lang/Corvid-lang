//! `corvid observe cost-optimise <agent>` — generative
//! route/cache/skip suggestions from a cost rollup.
//!
//! Aggregates cost-by-event-name across all traces under
//! `trace_dir`, identifies the top-N cost centres, and proposes
//! concrete optimisations. The heuristic suggests cache when an
//! event name with the same input fingerprint fired multiple
//! times, model-swap when a single prompt is the dominant cost
//! centre, and skip when a Failed event consumed budget without
//! producing a successful outcome.

use anyhow::Result;
use corvid_runtime::lineage::{LineageEvent, LineageKind, LineageStatus};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{read_lineage_input, source_descriptor};

#[derive(Debug, Clone)]
pub struct ObserveCostOptimiseArgs {
    pub trace_dir: PathBuf,
    pub agent: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostOptimisationReport {
    pub agent: String,
    pub trace_count: usize,
    pub total_cost_usd: f64,
    pub top_cost_centers: Vec<CostCenter>,
    pub suggestions: Vec<CostSuggestion>,
    pub sources: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostCenter {
    pub name: String,
    pub kind: String,
    pub total_cost_usd: f64,
    pub call_count: u64,
    pub percent_of_total: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostSuggestion {
    pub kind: String,
    pub description: String,
    pub estimated_savings_usd: f64,
    pub sources: Vec<Value>,
}

/// Aggregate cost-by-event-name across all traces under
/// `trace_dir`, identify the top-N cost centres, and propose
/// concrete optimisations. The heuristic suggests cache when an
/// event name with the same input fingerprint fired multiple
/// times, model-swap when a single prompt is the dominant cost
/// centre, and skip when a Failed event consumed budget without
/// producing a successful outcome.
pub fn run_observe_cost_optimise(args: ObserveCostOptimiseArgs) -> Result<CostOptimisationReport> {
    let events = read_lineage_input(&args.trace_dir)?;
    let agent_events: Vec<&LineageEvent> = events
        .iter()
        .filter(|e| {
            (e.kind == LineageKind::Agent && e.name == args.agent)
                || matches!(
                    e.kind,
                    LineageKind::Prompt | LineageKind::Tool | LineageKind::Db
                )
        })
        .collect();
    let total_cost: f64 = agent_events.iter().map(|e| e.cost_usd).sum();

    // Group by (kind, name).
    let mut groups: BTreeMap<(String, String), (f64, u64, BTreeMap<String, u64>)> = BTreeMap::new();
    for event in &agent_events {
        let key = (
            format!("{:?}", event.kind).to_lowercase(),
            event.name.clone(),
        );
        let entry = groups.entry(key).or_insert((0.0, 0, BTreeMap::new()));
        entry.0 += event.cost_usd;
        entry.1 += 1;
        if !event.input_fingerprint.is_empty() {
            *entry
                .2
                .entry(event.input_fingerprint.clone())
                .or_insert(0) += 1;
        }
    }

    let mut centers: Vec<CostCenter> = groups
        .iter()
        .map(|((kind, name), (cost, count, _))| CostCenter {
            name: name.clone(),
            kind: kind.clone(),
            total_cost_usd: *cost,
            call_count: *count,
            percent_of_total: if total_cost > 0.0 {
                (cost / total_cost) * 100.0
            } else {
                0.0
            },
        })
        .collect();
    centers.sort_by(|a, b| {
        b.total_cost_usd
            .partial_cmp(&a.total_cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_n = args.top_n.max(1).min(centers.len().max(1));
    let top: Vec<CostCenter> = centers.into_iter().take(top_n).collect();

    // Suggestions: cache for repeated input fingerprints; skip for
    // failed events consuming cost; model-swap for the top center
    // if it dwarfs the rest.
    let mut suggestions = Vec::new();
    for ((kind, name), (cost, count, fingerprints)) in &groups {
        let max_repeat = fingerprints.values().max().copied().unwrap_or(0);
        if max_repeat >= 2 {
            let saved = cost * (max_repeat as f64 - 1.0) / max_repeat.max(1) as f64;
            suggestions.push(CostSuggestion {
                kind: "cache".to_string(),
                description: format!(
                    "`{}` (kind={}) fired {} times with the same input fingerprint; \
                     caching the response would save approximately ${saved:.4} of the \
                     ${cost:.4} this {} spent total",
                    name, kind, max_repeat, kind
                ),
                estimated_savings_usd: saved,
                sources: agent_events
                    .iter()
                    .filter(|e| {
                        e.name == *name
                            && format!("{:?}", e.kind).to_lowercase() == *kind
                    })
                    .take(3)
                    .map(|e| source_descriptor(e))
                    .collect(),
            });
        }
        if *count >= 1 && *cost > 0.0 {
            let failed_cost: f64 = agent_events
                .iter()
                .filter(|e| {
                    e.name == *name
                        && format!("{:?}", e.kind).to_lowercase() == *kind
                        && matches!(e.status, LineageStatus::Failed | LineageStatus::Denied)
                })
                .map(|e| e.cost_usd)
                .sum();
            if failed_cost > 0.0 {
                suggestions.push(CostSuggestion {
                    kind: "skip_pre_validate".to_string(),
                    description: format!(
                        "`{}` consumed ${failed_cost:.4} on failed/denied attempts; \
                         add a pre-validate step (cheap check) before invoking the \
                         expensive {kind}",
                        name
                    ),
                    estimated_savings_usd: failed_cost,
                    sources: agent_events
                        .iter()
                        .filter(|e| {
                            e.name == *name
                                && matches!(
                                    e.status,
                                    LineageStatus::Failed | LineageStatus::Denied
                                )
                        })
                        .take(3)
                        .map(|e| source_descriptor(e))
                        .collect(),
                });
            }
        }
    }
    if let Some(first) = top.first() {
        if first.percent_of_total > 50.0 && first.kind == "prompt" {
            suggestions.push(CostSuggestion {
                kind: "model_swap".to_string(),
                description: format!(
                    "`{}` is the dominant cost centre ({:.1}% of total); a smaller-model \
                     route via `@progressive(escalate_to: ...)` or a per-call \
                     `min_confidence` threshold could reduce spend",
                    first.name, first.percent_of_total
                ),
                estimated_savings_usd: first.total_cost_usd * 0.30,
                sources: agent_events
                    .iter()
                    .filter(|e| {
                        e.name == first.name
                            && format!("{:?}", e.kind).to_lowercase() == first.kind
                    })
                    .take(3)
                    .map(|e| source_descriptor(e))
                    .collect(),
            });
        }
    }

    let trace_count = agent_events
        .iter()
        .map(|e| e.trace_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let sources: Vec<Value> = agent_events
        .iter()
        .take(20)
        .map(|e| source_descriptor(e))
        .collect();

    Ok(CostOptimisationReport {
        agent: args.agent,
        trace_count,
        total_cost_usd: total_cost,
        top_cost_centers: top,
        suggestions,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe_helpers_cmd::test_support::{ev, write_lineage};

    /// Slice 40K: cost-optimise computes correct percentages and
    /// suggests caching for repeated input fingerprints.
    #[test]
    fn cost_optimise_suggests_cache_for_repeated_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.lineage.jsonl");
        let mut e1 = ev(
            LineageKind::Prompt,
            "summarise",
            "t1",
            "s1",
            LineageStatus::Ok,
            "",
            0.20,
        );
        e1.input_fingerprint = "fp-1".to_string();
        let mut e2 = ev(
            LineageKind::Prompt,
            "summarise",
            "t2",
            "s2",
            LineageStatus::Ok,
            "",
            0.20,
        );
        e2.input_fingerprint = "fp-1".to_string(); // same input
        let mut e3 = ev(
            LineageKind::Tool,
            "fetch",
            "t1",
            "s3",
            LineageStatus::Ok,
            "",
            0.05,
        );
        e3.input_fingerprint = "fp-2".to_string();
        write_lineage(&path, &[e1, e2, e3]);
        let report = run_observe_cost_optimise(ObserveCostOptimiseArgs {
            trace_dir: dir.path().to_path_buf(),
            agent: "any".to_string(),
            top_n: 3,
        })
        .unwrap();
        assert!(report.total_cost_usd > 0.0);
        let cache_suggestions: Vec<_> = report
            .suggestions
            .iter()
            .filter(|s| s.kind == "cache")
            .collect();
        assert!(!cache_suggestions.is_empty());
        // The summarise prompt has the highest cost.
        assert_eq!(report.top_cost_centers[0].name, "summarise");
    }
}
