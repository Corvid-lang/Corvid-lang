pub mod otel;
pub mod prometheus;

use crate::alerts::Alert;
use crate::replay_pool::ShadowReplayOutcome;
use async_trait::async_trait;

#[async_trait]
pub trait ExportSink: Send + Sync {
    async fn record_outcome(&self, outcome: &ShadowReplayOutcome, alerts: &[Alert]) -> anyhow::Result<()>;
}

pub use otel::{OtelExporter, OtelSpanRecord};
pub use prometheus::{PrometheusExporter, PrometheusSnapshot};
