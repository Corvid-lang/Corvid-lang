//! Routing-report ingestion path — load trace events, optionally
//! filter by since timestamp / since-commit (which shells out to
//! `git log -1 --format=%ct <sha>` to resolve the commit's
//! committer timestamp), then aggregate per-model usage, the
//! escalation / exhaustion ladder, ensemble winners,
//! A/B-rollout cohort splits, and adversarial-stage outcomes
//! into the typed report rows.
//!
//! `event_ts_ms` is the trace-event timestamp extractor used by
//! the since-filter. The five `*Agg` structs are the per-axis
//! accumulators; they're consumed once at the end of
//! `build_report` to produce the typed `*Row` records.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use corvid_runtime::TraceEvent;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::{
    fmt_conf, model_label, percentile50, EscalationRow, ModelUsageRow, RoutingReport,
    RoutingReportOptions, StrategyRow,
};

#[derive(Default)]
struct ModelUsageAgg {
    count: u64,
    total_cost: f64,
    latencies: Vec<u64>,
}

#[derive(Default)]
struct EscalationAgg {
    model: Option<String>,
    entry_count: u64,
    escalation_count: u64,
    exhaustion_count: u64,
}

#[derive(Default)]
struct EnsembleAgg {
    winners: BTreeMap<String, u64>,
    agreement_sum: f64,
    count: u64,
}

#[derive(Default)]
struct RolloutAgg {
    winners: BTreeMap<String, u64>,
    total: u64,
    variant_count: u64,
    declared_pct: f64,
}

#[derive(Default)]
struct AdversarialAgg {
    total: u64,
    contradictions: u64,
}

pub fn build_report(opts: RoutingReportOptions<'_>) -> Result<RoutingReport> {
    let since_ms = resolve_since_ms(opts.since, opts.since_commit)?;
    let events = load_trace_events(opts.trace_dir, since_ms)?;

    let mut usage: HashMap<(String, String), ModelUsageAgg> = HashMap::new();
    let mut pending_llm: HashMap<(String, String, String), VecDeque<u64>> = HashMap::new();
    let mut escalations: HashMap<(String, usize), EscalationAgg> = HashMap::new();
    let mut ensembles: HashMap<String, EnsembleAgg> = HashMap::new();
    let mut rollouts: HashMap<String, RolloutAgg> = HashMap::new();
    let mut adversarial: HashMap<String, AdversarialAgg> = HashMap::new();

    for event in &events {
        match event {
            TraceEvent::ModelSelected {
                prompt,
                model,
                model_version,
                cost_estimate,
                stage_index,
                ..
            } => {
                let model_label = model_label(model, model_version.as_deref());
                let row = usage
                    .entry((prompt.clone(), model_label.clone()))
                    .or_default();
                row.count += 1;
                row.total_cost += cost_estimate;
                if let Some(stage) = stage_index {
                    let agg = escalations.entry((prompt.clone(), *stage)).or_default();
                    agg.entry_count += 1;
                    if agg.model.is_none() {
                        agg.model = Some(model_label);
                    }
                }
            }
            TraceEvent::LlmCall {
                ts_ms,
                run_id,
                prompt,
                model,
                model_version,
                ..
            } => {
                if let Some(model) = model {
                    let model_label = model_label(model, model_version.as_deref());
                    pending_llm
                        .entry((run_id.clone(), prompt.clone(), model_label))
                        .or_default()
                        .push_back(*ts_ms);
                }
            }
            TraceEvent::LlmResult {
                ts_ms,
                run_id,
                prompt,
                model,
                model_version,
                ..
            } => {
                if let Some(model) = model {
                    let model_label = model_label(model, model_version.as_deref());
                    let key = (run_id.clone(), prompt.clone(), model_label.clone());
                    if let Some(queue) = pending_llm.get_mut(&key) {
                        if let Some(start) = queue.pop_front() {
                            if let Some(row) = usage.get_mut(&(prompt.clone(), model_label)) {
                                row.latencies.push(ts_ms.saturating_sub(start));
                            }
                        }
                    }
                }
            }
            TraceEvent::ProgressiveEscalation {
                prompt, from_stage, ..
            } => {
                escalations
                    .entry((prompt.clone(), *from_stage))
                    .or_default()
                    .escalation_count += 1;
            }
            TraceEvent::ProgressiveExhausted { prompt, stages, .. } => {
                if let Some(last_stage) = stages.len().checked_sub(1) {
                    let agg = escalations.entry((prompt.clone(), last_stage)).or_default();
                    agg.exhaustion_count += 1;
                    if agg.model.is_none() {
                        agg.model = stages.get(last_stage).cloned();
                    }
                }
            }
            TraceEvent::StreamUpgrade { prompt, .. } => {
                escalations
                    .entry((prompt.clone(), 0))
                    .or_default()
                    .escalation_count += 1;
            }
            TraceEvent::AbVariantChosen {
                prompt,
                variant,
                baseline,
                rollout_pct,
                chosen,
                ..
            } => {
                let agg = rollouts.entry(prompt.clone()).or_default();
                agg.total += 1;
                agg.declared_pct = *rollout_pct;
                *agg.winners.entry(chosen.clone()).or_insert(0) += 1;
                let is_variant = chosen == variant;
                if is_variant {
                    agg.variant_count += 1;
                } else {
                    agg.winners.entry(baseline.clone()).or_insert(0);
                }
            }
            TraceEvent::EnsembleVote {
                prompt,
                winner,
                agreement_rate,
                ..
            } => {
                let agg = ensembles.entry(prompt.clone()).or_default();
                agg.count += 1;
                agg.agreement_sum += agreement_rate;
                *agg.winners.entry(winner.clone()).or_insert(0) += 1;
            }
            TraceEvent::AdversarialPipelineCompleted {
                prompt,
                contradiction,
                ..
            } => {
                let agg = adversarial.entry(prompt.clone()).or_default();
                agg.total += 1;
                if *contradiction {
                    agg.contradictions += 1;
                }
            }
            TraceEvent::AdversarialContradiction { prompt, .. } => {
                let agg = adversarial.entry(prompt.clone()).or_default();
                if agg.total == 0 {
                    agg.total = 1;
                }
                if agg.contradictions == 0 {
                    agg.contradictions = 1;
                }
            }
            _ => {}
        }
    }

    let prompt_totals = usage.iter().fold(
        HashMap::<String, u64>::new(),
        |mut acc, ((prompt, _), row)| {
            *acc.entry(prompt.clone()).or_insert(0) += row.count;
            acc
        },
    );

    let mut model_usage: Vec<ModelUsageRow> = usage
        .into_iter()
        .map(|((prompt, model), agg)| {
            let share =
                agg.count as f64 / (*prompt_totals.get(&prompt).unwrap_or(&agg.count) as f64);
            let mean_cost = agg.total_cost / agg.count as f64;
            let p50_latency_ms = percentile50(&mut agg.latencies.clone());
            let recommendation = if share >= 0.80 {
                format!(
                    "promote {model} to default — used {:.0}% of calls at {} confidence",
                    share * 100.0,
                    fmt_conf(None)
                )
            } else if share <= 0.10 {
                format!(
                    "underutilized — {model} handles only {:.0}% of calls",
                    share * 100.0
                )
            } else {
                "healthy".to_string()
            };
            let healthy = share > 0.10 && share < 0.80;
            ModelUsageRow {
                prompt,
                model,
                call_count: agg.count,
                mean_cost,
                mean_confidence: None,
                p50_latency_ms,
                recommendation,
                healthy,
            }
        })
        .collect();
    model_usage.sort_by(|a, b| (&a.prompt, &a.model).cmp(&(&b.prompt, &b.model)));

    let mut escalation_patterns: Vec<EscalationRow> = escalations
        .into_iter()
        .map(|((prompt, stage), agg)| {
            let escalated_pct = if agg.entry_count == 0 {
                0.0
            } else {
                (agg.escalation_count as f64 / agg.entry_count as f64) * 100.0
            };
            let (recommendation, healthy) = if agg.entry_count > 0
                && agg.exhaustion_count == agg.entry_count
            {
                (
                    "terminal always reached — promote cheaper model out of the chain".to_string(),
                    false,
                )
            } else if stage == 0 && agg.entry_count > 0 && agg.escalation_count == 0 {
                (
                    "never escalates — demote primary to a cheaper model".to_string(),
                    false,
                )
            } else if escalated_pct >= 80.0 {
                (
                    "frequently escalates — strengthen or remove this stage".to_string(),
                    false,
                )
            } else {
                ("healthy".to_string(), true)
            };
            EscalationRow {
                prompt,
                stage,
                model: agg.model,
                entry_count: agg.entry_count,
                escalation_count: agg.escalation_count,
                exhaustion_count: agg.exhaustion_count,
                escalated_pct,
                recommendation,
                healthy,
            }
        })
        .collect();
    escalation_patterns.sort_by(|a, b| (&a.prompt, a.stage).cmp(&(&b.prompt, b.stage)));

    let mut strategy_rows = Vec::new();
    for (prompt, agg) in rollouts {
        let observed_share = if agg.total == 0 {
            0.0
        } else {
            (agg.variant_count as f64 / agg.total as f64) * 100.0
        };
        let declared = agg.declared_pct / 100.0;
        let observed = observed_share / 100.0;
        let sigma = if agg.total == 0 {
            0.0
        } else {
            (declared * (1.0 - declared) / agg.total as f64).sqrt()
        };
        let unhealthy = sigma > 0.0 && (observed - declared).abs() > 3.0 * sigma;
        let recommendation = if unhealthy {
            "observed rollout share drifts past sampling variance — inspect cohort assignment"
                .to_string()
        } else {
            "cohort ratio stable".to_string()
        };
        strategy_rows.push(StrategyRow {
            prompt,
            strategy: "rollout".to_string(),
            winner_distribution: agg.winners,
            agreement_rate_mean: None,
            contradiction_rate: None,
            observed_variant_share: Some(observed_share),
            declared_rollout_pct: Some(agg.declared_pct),
            recommendation,
            healthy: !unhealthy,
        });
    }
    for (prompt, agg) in ensembles {
        let mean_agreement = if agg.count == 0 {
            None
        } else {
            Some(agg.agreement_sum / agg.count as f64)
        };
        let dominant = agg
            .winners
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(winner, count)| (winner.clone(), *count));
        let (recommendation, healthy) = if let Some((winner, count)) = dominant {
            let share = count as f64 / agg.count.max(1) as f64;
            if share >= 0.80 {
                (
                    format!(
                        "promote {winner} to default — wins {:.0}% of votes",
                        share * 100.0
                    ),
                    false,
                )
            } else if mean_agreement.unwrap_or(1.0) < 0.60 {
                (
                    "low ensemble agreement — prompt is unstable; tighten task or use stronger models".to_string(),
                    false,
                )
            } else {
                ("healthy".to_string(), true)
            }
        } else {
            ("healthy".to_string(), true)
        };
        strategy_rows.push(StrategyRow {
            prompt,
            strategy: "ensemble".to_string(),
            winner_distribution: agg.winners,
            agreement_rate_mean: mean_agreement,
            contradiction_rate: None,
            observed_variant_share: None,
            declared_rollout_pct: None,
            recommendation,
            healthy,
        });
    }
    for (prompt, agg) in adversarial {
        let contradiction_rate = if agg.total == 0 {
            None
        } else {
            Some(agg.contradictions as f64 / agg.total as f64)
        };
        let healthy = contradiction_rate.unwrap_or(0.0) < 0.20;
        let recommendation = if healthy {
            "low contradiction rate — consider whether full adversarial review is worth the cost"
                .to_string()
        } else {
            "high contradiction rate — keep the adversarial pipeline; proposer is catching real errors".to_string()
        };
        let mut winners = BTreeMap::new();
        winners.insert("contradiction".to_string(), agg.contradictions);
        winners.insert(
            "accepted".to_string(),
            agg.total.saturating_sub(agg.contradictions),
        );
        strategy_rows.push(StrategyRow {
            prompt,
            strategy: "adversarial".to_string(),
            winner_distribution: winners,
            agreement_rate_mean: None,
            contradiction_rate,
            observed_variant_share: None,
            declared_rollout_pct: None,
            recommendation,
            healthy,
        });
    }
    strategy_rows.sort_by(|a, b| (&a.prompt, &a.strategy).cmp(&(&b.prompt, &b.strategy)));

    let healthy = model_usage.iter().all(|row| row.healthy)
        && escalation_patterns.iter().all(|row| row.healthy)
        && strategy_rows.iter().all(|row| row.healthy);

    Ok(RoutingReport {
        trace_dir: opts.trace_dir.display().to_string(),
        since_ms,
        healthy,
        model_usage,
        escalation_patterns,
        strategy_rows,
    })
}

fn load_trace_events(trace_dir: &Path, since_ms: Option<u64>) -> Result<Vec<TraceEvent>> {
    let mut events = Vec::new();
    for entry in fs::read_dir(trace_dir)
        .with_context(|| format!("failed to read trace dir `{}`", trace_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let body = fs::read_to_string(&path)
            .with_context(|| format!("failed to read trace file `{}`", path.display()))?;
        for (idx, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: TraceEvent = serde_json::from_str(line).with_context(|| {
                format!("invalid trace event at {}:{}", path.display(), idx + 1)
            })?;
            if since_ms
                .map(|cutoff| event_ts_ms(&event) >= cutoff)
                .unwrap_or(true)
            {
                events.push(event);
            }
        }
    }
    events.sort_by_key(event_ts_ms);
    Ok(events)
}

fn resolve_since_ms(since: Option<&str>, since_commit: Option<&str>) -> Result<Option<u64>> {
    let mut cutoffs = Vec::new();
    if let Some(since) = since {
        let ts = OffsetDateTime::parse(since, &Rfc3339)
            .with_context(|| format!("invalid --since timestamp `{since}`; expected RFC3339"))?;
        cutoffs.push((ts.unix_timestamp_nanos() / 1_000_000) as u64);
    }
    if let Some(sha) = since_commit {
        let output = Command::new("git")
            .args(["show", "-s", "--format=%ct", sha])
            .output()
            .with_context(|| format!("failed to run `git show` for `{sha}`"))?;
        if !output.status.success() {
            bail!("failed to resolve commit `{sha}`");
        }
        let secs = String::from_utf8(output.stdout)
            .context("git returned non-utf8 commit timestamp")?
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid git timestamp for `{sha}`"))?;
        cutoffs.push(secs * 1000);
    }
    Ok(cutoffs.into_iter().max())
}

fn event_ts_ms(event: &TraceEvent) -> u64 {
    match event {
        TraceEvent::SchemaHeader { ts_ms, .. }
        | TraceEvent::RunStarted { ts_ms, .. }
        | TraceEvent::RunCompleted { ts_ms, .. }
        | TraceEvent::ToolCall { ts_ms, .. }
        | TraceEvent::ToolResult { ts_ms, .. }
        | TraceEvent::LlmCall { ts_ms, .. }
        | TraceEvent::LlmResult { ts_ms, .. }
        | TraceEvent::PromptCache { ts_ms, .. }
        | TraceEvent::ApprovalRequest { ts_ms, .. }
        | TraceEvent::ApprovalDecision { ts_ms, .. }
        | TraceEvent::ApprovalResponse { ts_ms, .. }
        | TraceEvent::ApprovalTokenIssued { ts_ms, .. }
        | TraceEvent::ApprovalScopeViolation { ts_ms, .. }
        | TraceEvent::HumanInputRequest { ts_ms, .. }
        | TraceEvent::HumanInputResponse { ts_ms, .. }
        | TraceEvent::HumanChoiceRequest { ts_ms, .. }
        | TraceEvent::HumanChoiceResponse { ts_ms, .. }
        | TraceEvent::HostEvent { ts_ms, .. }
        | TraceEvent::SeedRead { ts_ms, .. }
        | TraceEvent::ClockRead { ts_ms, .. }
        | TraceEvent::ModelSelected { ts_ms, .. }
        | TraceEvent::ProgressiveEscalation { ts_ms, .. }
        | TraceEvent::ProgressiveExhausted { ts_ms, .. }
        | TraceEvent::StreamUpgrade { ts_ms, .. }
        | TraceEvent::AbVariantChosen { ts_ms, .. }
        | TraceEvent::EnsembleVote { ts_ms, .. }
        | TraceEvent::AdversarialPipelineCompleted { ts_ms, .. }
        | TraceEvent::AdversarialContradiction { ts_ms, .. }
        | TraceEvent::ProvenanceEdge { ts_ms, .. } => *ts_ms,
    }
}
