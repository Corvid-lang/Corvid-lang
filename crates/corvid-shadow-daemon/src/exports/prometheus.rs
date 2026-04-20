use super::ExportSink;
use crate::alerts::{Alert, AlertKind};
use crate::replay_pool::ShadowReplayOutcome;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Default)]
pub struct PrometheusSnapshot {
    pub divergences_total: HashMap<(String, String), u64>,
    pub invariant_violations_total: HashMap<(String, String), u64>,
    pub trust_tier_observed: HashMap<(String, String), u64>,
}

#[derive(Clone)]
pub struct PrometheusExporter {
    inner: Arc<Mutex<PrometheusSnapshot>>,
    bind_addr: String,
    _server: Arc<JoinHandle<()>>,
}

impl PrometheusExporter {
    pub async fn bind(bind_addr: &str) -> Result<Self> {
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?.to_string();
        let inner = Arc::new(Mutex::new(PrometheusSnapshot::default()));
        let shared = inner.clone();
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request_buf = [0_u8; 1024];
                let _ = socket.read(&mut request_buf).await;
                let body = render_metrics(&shared.lock().unwrap());
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; version=0.0.4\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        Ok(Self {
            inner,
            bind_addr: local_addr,
            _server: Arc::new(server),
        })
    }

    pub fn snapshot(&self) -> PrometheusSnapshot {
        self.inner.lock().unwrap().clone()
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }
}

#[async_trait]
impl ExportSink for PrometheusExporter {
    async fn record_outcome(&self, outcome: &ShadowReplayOutcome, alerts: &[Alert]) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let trust = format!("{:?}", outcome.shadow_dimensions.trust_tier);
        *inner
            .trust_tier_observed
            .entry((outcome.agent.clone(), trust))
            .or_insert(0) += 1;
        for alert in alerts {
            let dimension = alert
                .payload
                .get("dimension")
                .and_then(|value| value.as_str())
                .or_else(|| alert.payload.get("invariant").and_then(|value| value.as_str()))
                .unwrap_or(match alert.kind {
                    AlertKind::Dimension => "dimension",
                    AlertKind::Provenance => "provenance",
                    AlertKind::Counterfactual => "counterfactual",
                    AlertKind::Consensus => "consensus",
                    AlertKind::Invariant => "invariant",
                })
                .to_string();
            *inner
                .divergences_total
                .entry((alert.agent.clone(), dimension.clone()))
                .or_insert(0) += 1;
            if alert.kind == AlertKind::Invariant {
                *inner
                    .invariant_violations_total
                    .entry((alert.agent.clone(), dimension))
                    .or_insert(0) += 1;
            }
        }
        Ok(())
    }
}

fn render_metrics(snapshot: &PrometheusSnapshot) -> String {
    let mut body = String::new();
    body.push_str("# HELP corvid_shadow_divergences_total Shadow replay divergences by agent and dimension\n");
    body.push_str("# TYPE corvid_shadow_divergences_total counter\n");
    for ((agent, dimension), count) in &snapshot.divergences_total {
        body.push_str(&format!(
            "corvid_shadow_divergences_total{{agent=\"{}\",dimension=\"{}\"}} {}\n",
            agent, dimension, count
        ));
    }
    body.push_str("# HELP corvid_shadow_invariant_violations_total Runtime violations of compile-time invariants\n");
    body.push_str("# TYPE corvid_shadow_invariant_violations_total counter\n");
    for ((agent, invariant), count) in &snapshot.invariant_violations_total {
        body.push_str(&format!(
            "corvid_shadow_invariant_violations_total{{agent=\"{}\",invariant=\"{}\"}} {}\n",
            agent, invariant, count
        ));
    }
    body.push_str("# HELP corvid_shadow_trust_tier_observed Observed trust-tier distribution\n");
    body.push_str("# TYPE corvid_shadow_trust_tier_observed histogram\n");
    for ((agent, tier), count) in &snapshot.trust_tier_observed {
        body.push_str(&format!(
            "corvid_shadow_trust_tier_observed{{agent=\"{}\",tier=\"{}\"}} {}\n",
            agent, tier, count
        ));
    }
    body
}
