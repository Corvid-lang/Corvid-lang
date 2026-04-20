use crate::alerts::{self, Alert, AlertSink, MemoryAlertSink};
use crate::config::{load_config, DaemonConfig};
use crate::enrollment::{EnrollmentAction, EnrollmentManager};
use crate::exports::{ExportSink, OtelExporter, PrometheusExporter};
use crate::replay_pool::{
    InterpreterShadowExecutor, ReplayPool, ShadowExecutionMode, ShadowReplayExecutor,
};
use crate::subscribe::{FileWatchSubscription, TraceSubscription};
use anyhow::{Context, Result};
use corvid_runtime::now_ms;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShadowDaemonStatus {
    pub running: bool,
    pub queue_depth: usize,
    pub processed_traces: usize,
    pub alert_counts: HashMap<String, u64>,
}

#[derive(Clone)]
pub struct ShadowDaemonHandle {
    shutdown: Arc<Notify>,
    status: Arc<Mutex<ShadowDaemonStatus>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl ShadowDaemonHandle {
    pub async fn shutdown(&self) {
        self.shutdown.notify_waiters();
        self.status.lock().await.running = false;
        if let Some(task) = self.task.lock().await.as_ref() {
            task.abort();
        }
    }

    pub async fn status(&self) -> ShadowDaemonStatus {
        self.status.lock().await.clone()
    }

    pub async fn wait(&self) {
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

pub struct ShadowDaemon {
    config: DaemonConfig,
    pool: ReplayPool,
    subscription: Box<dyn TraceSubscription>,
    alert_sink: Arc<dyn AlertSink>,
    export_sinks: Vec<Arc<dyn ExportSink>>,
    enrollment: EnrollmentManager,
    status: Arc<Mutex<ShadowDaemonStatus>>,
    shutdown: Arc<Notify>,
    dimension_engine: alerts::dimension::DimensionAlertEngine,
}

impl ShadowDaemon {
    pub fn new(
        config: DaemonConfig,
        executor: Arc<dyn ShadowReplayExecutor>,
        subscription: Box<dyn TraceSubscription>,
        alert_sink: Arc<dyn AlertSink>,
        export_sinks: Vec<Arc<dyn ExportSink>>,
    ) -> Self {
        Self {
            pool: ReplayPool::new(executor, config.daemon.max_concurrent_replays),
            enrollment: EnrollmentManager::new(config.enrollment.clone()),
            config,
            subscription,
            alert_sink,
            export_sinks,
            status: Arc::new(Mutex::new(ShadowDaemonStatus {
                running: true,
                ..ShadowDaemonStatus::default()
            })),
            shutdown: Arc::new(Notify::new()),
            dimension_engine: alerts::dimension::DimensionAlertEngine::new(),
        }
    }

    pub fn status_handle(&self) -> Arc<Mutex<ShadowDaemonStatus>> {
        self.status.clone()
    }

    pub fn shutdown_signal(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    self.status.lock().await.running = false;
                    break;
                }
                next = self.subscription.next() => {
                    let Some(trace_path) = next else {
                        self.status.lock().await.running = false;
                        break;
                    };
                    self.process_trace(trace_path).await;
                }
            }
        }
    }

    async fn process_trace(&mut self, trace_path: PathBuf) {
        let base = match self.pool.execute(&trace_path, ShadowExecutionMode::Replay).await {
            Ok(outcome) => outcome,
            Err(err) => {
                let alert = error_alert(&trace_path, err.to_string());
                let _ = self.alert_sink.emit(alert.clone()).await;
                let _ = self.write_alert_log(&alert);
                let mut status = self.status.lock().await;
                status.processed_traces += 1;
                *status.alert_counts.entry("error".into()).or_insert(0) += 1;
                return;
            }
        };

        let mut alerts = Vec::new();
        alerts.extend(
            self.dimension_engine
                .evaluate(&self.config.alerts.dimension, &base),
        );
        alerts.extend(alerts::provenance::evaluate(
            &self.config.alerts.provenance,
            &base,
        ));
        alerts.extend(alerts::invariant::evaluate(&base));

        if alerts::counterfactual::should_sample(&trace_path, &self.config.alerts.counterfactual) {
            for mutation in alerts::counterfactual::propose_mutations(
                &base,
                &self.config.alerts.counterfactual,
            ) {
                if let Ok(mutated) = self
                    .pool
                    .execute(&trace_path, ShadowExecutionMode::Mutation(mutation.clone()))
                    .await
                {
                    if let Some(alert) =
                        alerts::counterfactual::analyze_mutation_outcome(&base, &mutated, &mutation)
                    {
                        alerts.push(alert);
                    }
                }
            }
        }

        if alerts::consensus::should_sample(&trace_path, &self.config.alerts.consensus) {
            let mut by_model = HashMap::new();
            for model in &self.config.alerts.consensus.models {
                if let Ok(outcome) = self
                    .pool
                    .execute(
                        &trace_path,
                        ShadowExecutionMode::Differential {
                            model: model.clone(),
                        },
                    )
                    .await
                {
                    by_model.insert(model.clone(), outcome);
                }
            }
            if let Some(alert) =
                alerts::consensus::evaluate(&self.config.alerts.consensus, &base, &by_model)
            {
                alerts.push(alert);
            }
        }

        for alert in &alerts {
            let _ = self.alert_sink.emit(alert.clone()).await;
            let _ = self.write_alert_log(alert);
            for sink in &self.export_sinks {
                let _ = sink.record_outcome(&base, std::slice::from_ref(alert)).await;
            }
            if let Ok(Some(action)) = self.enrollment.maybe_auto_enroll(alert) {
                let _ = action;
            }
        }

        let mut status = self.status.lock().await;
        status.processed_traces += 1;
        for alert in alerts {
            *status
                .alert_counts
                .entry(format!("{:?}", alert.kind).to_ascii_lowercase())
                .or_insert(0) += 1;
        }
    }

    fn write_alert_log(&self, alert: &Alert) -> Result<()> {
        if let Some(parent) = self.config.daemon.alert_log.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.daemon.alert_log)?;
        use std::io::Write;
        writeln!(file, "{}", serde_json::to_string(alert)?)?;
        Ok(())
    }
}

pub async fn start_daemon(config_path: &Path) -> Result<ShadowDaemonHandle> {
    let (config, warnings) = load_config(config_path)?;
    for warning in warnings {
        eprintln!("warning: {warning}");
    }

    let executor = Arc::new(InterpreterShadowExecutor::from_program_path(
        &config.daemon.ir_path,
    )?);
    let subscription = Box::new(
        FileWatchSubscription::watch(&config.daemon.trace_dir, config.subscribe.debounce_ms)
            .map_err(anyhow::Error::msg)?,
    );
    let alert_sink = Arc::new(MemoryAlertSink::new()) as Arc<dyn AlertSink>;

    let mut export_sinks: Vec<Arc<dyn ExportSink>> = Vec::new();
    if let Some(prom) = &config.exports.prometheus {
        export_sinks.push(Arc::new(PrometheusExporter::bind(&prom.bind_addr).await?));
    }
    if let Some(otel) = &config.exports.otel {
        export_sinks.push(Arc::new(OtelExporter::new(&otel.endpoint)));
    }

    let daemon = ShadowDaemon::new(config, executor, subscription, alert_sink, export_sinks);
    let status = daemon.status_handle();
    let shutdown = daemon.shutdown_signal();
    let task = tokio::spawn(daemon.run());
    Ok(ShadowDaemonHandle {
        shutdown,
        status,
        task: Arc::new(Mutex::new(Some(task))),
    })
}

pub async fn ack_trace(
    trace_path: &Path,
    reason: &str,
    target_corpus_dir: &Path,
) -> Result<EnrollmentAction> {
    let manager = EnrollmentManager::new(crate::config::EnrollmentConfig {
        target_corpus_dir: target_corpus_dir.to_path_buf(),
        auto_enroll: false,
        auto_enroll_on_trust_drop: false,
        auto_enroll_on_budget_overrun: false,
    });
    manager.enroll(trace_path, reason, None)
}

pub fn dump_alerts(alert_log: &Path, since: Option<&str>) -> Result<Vec<Alert>> {
    let body = std::fs::read_to_string(alert_log)
        .with_context(|| format!("failed to read alert log `{}`", alert_log.display()))?;
    let cutoff = since.and_then(parse_rfc3339_ms);
    let mut alerts = Vec::new();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let alert: Alert = serde_json::from_str(line)?;
        if cutoff.map(|cutoff| alert.ts_ms >= cutoff).unwrap_or(true) {
            alerts.push(alert);
        }
    }
    Ok(alerts)
}

fn parse_rfc3339_ms(input: &str) -> Option<u64> {
    let dt = time::OffsetDateTime::parse(input, &time::format_description::well_known::Rfc3339)
        .ok()?;
    Some((dt.unix_timestamp_nanos() / 1_000_000) as u64)
}

fn error_alert(trace_path: &Path, message: String) -> Alert {
    Alert {
        ts_ms: now_ms(),
        severity: alerts::AlertSeverity::Critical,
        kind: alerts::AlertKind::Invariant,
        agent: "<unknown>".into(),
        trace_path: trace_path.to_path_buf(),
        summary: "shadow replay failed before alert analysis".into(),
        payload: serde_json::json!({ "error": message }),
    }
}
