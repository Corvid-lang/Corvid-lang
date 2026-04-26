use crate::llm::ProviderHealth;
use crate::usage::LlmUsageRecord;

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
}
