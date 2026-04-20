use super::ExportSink;
use crate::alerts::Alert;
use crate::replay_pool::ShadowReplayOutcome;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelSpanRecord {
    pub name: String,
    pub attributes: serde_json::Value,
}

#[derive(Clone)]
pub struct OtelExporter {
    endpoint: String,
    client: reqwest::Client,
}

impl OtelExporter {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ExportSink for OtelExporter {
    async fn record_outcome(&self, outcome: &ShadowReplayOutcome, alerts: &[Alert]) -> Result<()> {
        let span = OtelSpanRecord {
            name: format!("corvid.shadow.{}", outcome.agent),
            attributes: serde_json::json!({
                "corvid.agent": outcome.agent,
                "corvid.trace_path": outcome.trace_path,
                "corvid.trust_tier": format!("{:?}", outcome.shadow_dimensions.trust_tier),
                "corvid.budget.declared": outcome.shadow_dimensions.budget_declared,
                "corvid.latency_ms": outcome.shadow_dimensions.latency_ms,
                "corvid.alerts": alerts,
            }),
        };
        let _ = self.client.post(&self.endpoint).json(&span).send().await;
        Ok(())
    }
}
