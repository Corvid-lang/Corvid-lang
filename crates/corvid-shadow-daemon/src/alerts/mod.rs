pub mod consensus;
pub mod counterfactual;
pub mod dimension;
pub mod invariant;
pub mod provenance;

use crate::replay_pool::ShadowReplayOutcome;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertKind {
    Dimension,
    Provenance,
    Counterfactual,
    Consensus,
    Invariant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub ts_ms: u64,
    pub severity: AlertSeverity,
    pub kind: AlertKind,
    pub agent: String,
    pub trace_path: PathBuf,
    pub summary: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ShadowAlertContext {
    pub trace_path: PathBuf,
    pub outcome: ShadowReplayOutcome,
}

#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn emit(&self, alert: Alert) -> anyhow::Result<()>;
}

#[derive(Clone, Default)]
pub struct MemoryAlertSink {
    alerts: Arc<Mutex<Vec<Alert>>>,
}

impl MemoryAlertSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alerts(&self) -> Vec<Alert> {
        self.alerts.lock().unwrap().clone()
    }
}

#[async_trait]
impl AlertSink for MemoryAlertSink {
    async fn emit(&self, alert: Alert) -> anyhow::Result<()> {
        self.alerts.lock().unwrap().push(alert);
        Ok(())
    }
}
