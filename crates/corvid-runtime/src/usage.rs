use crate::llm::TokenUsage;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub struct LlmUsageRecord {
    pub ts_ms: u64,
    pub prompt: String,
    pub model: String,
    pub provider: Option<String>,
    pub adapter: Option<String>,
    pub privacy_tier: Option<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub local: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmUsageTotals {
    pub calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Clone, Default)]
pub struct LlmUsageLedger {
    records: Arc<Mutex<Vec<LlmUsageRecord>>>,
}

impl LlmUsageLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, record: LlmUsageRecord) {
        self.records.lock().unwrap().push(record);
    }

    pub fn records(&self) -> Vec<LlmUsageRecord> {
        self.records.lock().unwrap().clone()
    }

    pub fn totals_by_provider(&self) -> BTreeMap<String, LlmUsageTotals> {
        let records = self.records.lock().unwrap();
        let mut totals = BTreeMap::new();
        for record in records.iter() {
            let key = record
                .provider
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let entry = totals.entry(key).or_insert_with(LlmUsageTotals::default);
            entry.calls += 1;
            entry.prompt_tokens += record.prompt_tokens;
            entry.completion_tokens += record.completion_tokens;
            entry.total_tokens += record.total_tokens;
            if record.cost_usd.is_finite() && record.cost_usd > 0.0 {
                entry.cost_usd += record.cost_usd;
            }
        }
        totals
    }
}

pub fn normalized_total_tokens(usage: TokenUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens as u64
    } else {
        usage.prompt_tokens as u64 + usage.completion_tokens as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_ledger_totals_by_provider() {
        let ledger = LlmUsageLedger::new();
        ledger.record(LlmUsageRecord {
            ts_ms: 1,
            prompt: "a".to_string(),
            model: "gpt".to_string(),
            provider: Some("openai".to_string()),
            adapter: Some("openai".to_string()),
            privacy_tier: Some("hosted".to_string()),
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            cost_usd: 0.25,
            local: false,
        });
        ledger.record(LlmUsageRecord {
            ts_ms: 2,
            prompt: "b".to_string(),
            model: "llama".to_string(),
            provider: Some("ollama".to_string()),
            adapter: Some("ollama".to_string()),
            privacy_tier: Some("local".to_string()),
            prompt_tokens: 3,
            completion_tokens: 4,
            total_tokens: 7,
            cost_usd: 0.0,
            local: true,
        });

        let totals = ledger.totals_by_provider();
        assert_eq!(totals["openai"].calls, 1);
        assert_eq!(totals["openai"].total_tokens, 15);
        assert_eq!(totals["openai"].cost_usd, 0.25);
        assert_eq!(totals["ollama"].calls, 1);
        assert_eq!(totals["ollama"].cost_usd, 0.0);
    }

    #[test]
    fn normalized_total_tokens_falls_back_to_input_plus_output() {
        assert_eq!(
            normalized_total_tokens(TokenUsage {
                prompt_tokens: 4,
                completion_tokens: 6,
                total_tokens: 0,
            }),
            10
        );
    }
}
