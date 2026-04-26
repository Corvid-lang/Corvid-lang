use crate::llm::ProviderHealth;
use crate::usage::LlmUsageRecord;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeObservationSummary {
    pub llm_calls: u64,
    pub local_llm_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub provider_count: u64,
    pub degraded_provider_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservation {
    pub adapter: String,
    pub degraded: bool,
    pub consecutive_failures: u64,
    pub last_success_ms: Option<u64>,
    pub last_failure_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyObservation {
    pub name: String,
    pub count: u64,
    pub min_ms: u64,
    pub max_ms: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalObservationSummary {
    pub label: String,
    pub approved: u64,
    pub denied: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteObservationSummary {
    pub from_model: String,
    pub to_model: String,
    pub count: u64,
}

pub fn runtime_observation_summary(
    usage: &[LlmUsageRecord],
    health: &[ProviderHealth],
) -> RuntimeObservationSummary {
    RuntimeObservationSummary {
        llm_calls: usage.len() as u64,
        local_llm_calls: usage.iter().filter(|record| record.local).count() as u64,
        prompt_tokens: usage.iter().map(|record| record.prompt_tokens).sum(),
        completion_tokens: usage.iter().map(|record| record.completion_tokens).sum(),
        total_tokens: usage.iter().map(|record| record.total_tokens).sum(),
        cost_usd: usage
            .iter()
            .map(|record| record.cost_usd)
            .filter(|cost| cost.is_finite() && *cost > 0.0)
            .sum(),
        provider_count: health.len() as u64,
        degraded_provider_count: health.iter().filter(|provider| provider.degraded).count() as u64,
    }
}

pub fn provider_observations(health: &[ProviderHealth]) -> Vec<ProviderObservation> {
    health
        .iter()
        .map(|provider| ProviderObservation {
            adapter: provider.adapter.clone(),
            degraded: provider.degraded,
            consecutive_failures: provider.consecutive_failures,
            last_success_ms: provider.last_success_ms,
            last_failure_ms: provider.last_failure_ms,
        })
        .collect()
}

pub fn latency_histogram(name: impl Into<String>, samples_ms: &[u64]) -> LatencyObservation {
    let name = name.into();
    if samples_ms.is_empty() {
        return LatencyObservation {
            name,
            count: 0,
            min_ms: 0,
            max_ms: 0,
            p50_ms: 0,
            p95_ms: 0,
            p99_ms: 0,
        };
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_unstable();
    LatencyObservation {
        name,
        count: sorted.len() as u64,
        min_ms: sorted[0],
        max_ms: *sorted.last().unwrap_or(&0),
        p50_ms: percentile(&sorted, 50),
        p95_ms: percentile(&sorted, 95),
        p99_ms: percentile(&sorted, 99),
    }
}

pub fn approval_summary(label: impl Into<String>, approvals: &[bool]) -> ApprovalObservationSummary {
    let label = label.into();
    ApprovalObservationSummary {
        label,
        approved: approvals.iter().filter(|value| **value).count() as u64,
        denied: approvals.iter().filter(|value| !**value).count() as u64,
    }
}

pub fn route_summaries(routes: &[(String, String)]) -> Vec<RouteObservationSummary> {
    let mut counts: BTreeMap<(String, String), u64> = BTreeMap::new();
    for (from_model, to_model) in routes {
        *counts
            .entry((from_model.clone(), to_model.clone()))
            .or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|((from_model, to_model), count)| RouteObservationSummary {
            from_model,
            to_model,
            count,
        })
        .collect()
}

fn percentile(sorted: &[u64], percentile: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (((sorted.len() - 1) as u128) * (percentile as u128) / 100) as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_usage_and_degraded_providers() {
        let usage = vec![
            LlmUsageRecord {
                ts_ms: 1,
                prompt: "a".to_string(),
                model: "gpt".to_string(),
                provider: Some("openai".to_string()),
                adapter: Some("openai".to_string()),
                privacy_tier: Some("hosted".to_string()),
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cost_usd: 0.30,
                local: false,
            },
            LlmUsageRecord {
                ts_ms: 2,
                prompt: "b".to_string(),
                model: "llama".to_string(),
                provider: Some("ollama".to_string()),
                adapter: Some("ollama".to_string()),
                privacy_tier: Some("local".to_string()),
                prompt_tokens: 4,
                completion_tokens: 6,
                total_tokens: 10,
                cost_usd: 0.0,
                local: true,
            },
        ];
        let health = vec![
            ProviderHealth {
                adapter: "openai".to_string(),
                consecutive_failures: 0,
                last_success_ms: Some(10),
                last_failure_ms: None,
                degraded: false,
            },
            ProviderHealth {
                adapter: "ollama".to_string(),
                consecutive_failures: 2,
                last_success_ms: None,
                last_failure_ms: Some(12),
                degraded: true,
            },
        ];

        let summary = runtime_observation_summary(&usage, &health);
        assert_eq!(summary.llm_calls, 2);
        assert_eq!(summary.local_llm_calls, 1);
        assert_eq!(summary.total_tokens, 25);
        assert_eq!(summary.cost_usd, 0.30);
        assert_eq!(summary.provider_count, 2);
        assert_eq!(summary.degraded_provider_count, 1);
    }

    #[test]
    fn latency_histogram_summarizes_samples() {
        let summary = latency_histogram("llm.call", &[5, 10, 20, 25, 100]);
        assert_eq!(summary.count, 5);
        assert_eq!(summary.min_ms, 5);
        assert_eq!(summary.max_ms, 100);
        assert_eq!(summary.p50_ms, 20);
        assert_eq!(summary.p95_ms, 25);
        assert_eq!(summary.p99_ms, 25);
    }

    #[test]
    fn approval_and_route_summaries_aggregate_counts() {
        let approvals = approval_summary("charge-card", &[true, false, true, true]);
        assert_eq!(approvals.approved, 3);
        assert_eq!(approvals.denied, 1);

        let routes = route_summaries(&[
            ("gpt-4o-mini".to_string(), "gpt-4.1".to_string()),
            ("gpt-4o-mini".to_string(), "gpt-4.1".to_string()),
            ("gpt-4.1".to_string(), "gpt-4.1".to_string()),
        ]);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].count + routes[1].count, 3);
    }
}
