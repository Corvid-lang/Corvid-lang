use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DaemonConfig {
    pub daemon: DaemonSection,
    #[serde(default)]
    pub subscribe: SubscribeConfig,
    #[serde(default)]
    pub alerts: AlertsConfig,
    #[serde(default)]
    pub enrollment: EnrollmentConfig,
    #[serde(default)]
    pub exports: ExportConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DaemonSection {
    pub trace_dir: PathBuf,
    /// Phase 21 v1 still compiles from source at startup; when this path
    /// ends in `.cor`, the daemon treats it as the current program source.
    /// Serialized IR loading is reserved for a follow-up.
    pub ir_path: PathBuf,
    #[serde(default = "default_max_concurrent_replays")]
    pub max_concurrent_replays: usize,
    pub alert_log: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AlertLogConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SubscribeConfig {
    #[serde(default = "default_subscribe_kind")]
    pub kind: String,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
}

impl Default for SubscribeConfig {
    fn default() -> Self {
        Self {
            kind: default_subscribe_kind(),
            debounce_ms: default_debounce_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AlertsConfig {
    #[serde(default)]
    pub dimension: DimensionAlertConfig,
    #[serde(default)]
    pub provenance: ProvenanceAlertConfig,
    #[serde(default)]
    pub counterfactual: CounterfactualAlertConfig,
    #[serde(default)]
    pub consensus: ConsensusAlertConfig,
    #[serde(default)]
    pub invariant: InvariantAlertConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DimensionAlertConfig {
    #[serde(default)]
    pub trust: DimensionAlertTrustConfig,
    #[serde(default)]
    pub budget: DimensionAlertBudgetConfig,
    #[serde(default)]
    pub latency: DimensionAlertLatencyConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DimensionAlertTrustConfig {
    #[serde(default = "default_trust_fraction_below_autonomous")]
    pub threshold_fraction_below_autonomous: f64,
}

impl Default for DimensionAlertTrustConfig {
    fn default() -> Self {
        Self {
            threshold_fraction_below_autonomous: default_trust_fraction_below_autonomous(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DimensionAlertBudgetConfig {
    #[serde(default = "default_true")]
    pub alert_on_overrun: bool,
    #[serde(default = "default_burn_rate_days_runway")]
    pub burn_rate_alert_days_runway: u32,
}

impl Default for DimensionAlertBudgetConfig {
    fn default() -> Self {
        Self {
            alert_on_overrun: true,
            burn_rate_alert_days_runway: default_burn_rate_days_runway(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DimensionAlertLatencyConfig {
    #[serde(default)]
    pub p50_ms: Option<u64>,
    #[serde(default)]
    pub p99_ms: Option<u64>,
}

impl Default for DimensionAlertLatencyConfig {
    fn default() -> Self {
        Self {
            p50_ms: None,
            p99_ms: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProvenanceAlertConfig {
    #[serde(default = "default_true")]
    pub alert_on_reasoning_drift: bool,
    #[serde(default = "default_true")]
    pub alert_on_chain_break: bool,
}

impl Default for ProvenanceAlertConfig {
    fn default() -> Self {
        Self {
            alert_on_reasoning_drift: true,
            alert_on_chain_break: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CounterfactualAlertConfig {
    #[serde(default = "default_counterfactual_fraction")]
    pub sample_fraction: f64,
    #[serde(default = "default_max_mutations_per_trace")]
    pub max_mutations_per_trace: usize,
}

impl Default for CounterfactualAlertConfig {
    fn default() -> Self {
        Self {
            sample_fraction: default_counterfactual_fraction(),
            max_mutations_per_trace: default_max_mutations_per_trace(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConsensusAlertConfig {
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default = "default_consensus_min_agreement")]
    pub min_agreement: usize,
    #[serde(default = "default_consensus_fraction")]
    pub sample_fraction: f64,
}

impl Default for ConsensusAlertConfig {
    fn default() -> Self {
        Self {
            models: Vec::new(),
            min_agreement: default_consensus_min_agreement(),
            sample_fraction: default_consensus_fraction(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct InvariantAlertConfig {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EnrollmentConfig {
    #[serde(default = "default_corpus_dir")]
    pub target_corpus_dir: PathBuf,
    #[serde(default)]
    pub auto_enroll: bool,
    #[serde(default)]
    pub auto_enroll_on_trust_drop: bool,
    #[serde(default)]
    pub auto_enroll_on_budget_overrun: bool,
}

impl Default for EnrollmentConfig {
    fn default() -> Self {
        Self {
            target_corpus_dir: default_corpus_dir(),
            auto_enroll: false,
            auto_enroll_on_trust_drop: true,
            auto_enroll_on_budget_overrun: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ExportConfig {
    #[serde(default)]
    pub prometheus: Option<ExportPrometheusConfig>,
    #[serde(default)]
    pub otel: Option<ExportOtelConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExportPrometheusConfig {
    pub bind_addr: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExportOtelConfig {
    pub endpoint: String,
}

pub fn load_config(path: &Path) -> Result<(DaemonConfig, Vec<String>)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read daemon config `{}`", path.display()))?;
    let mut warnings = Vec::new();
    let deserializer = toml::Deserializer::new(&raw);
    let config: DaemonConfig = serde_ignored::deserialize(deserializer, |path| {
        warnings.push(format!("unknown config key `{path}`"));
    })
    .with_context(|| format!("failed to parse daemon config `{}`", path.display()))?;
    Ok((config, warnings))
}

fn default_max_concurrent_replays() -> usize {
    4
}

fn default_subscribe_kind() -> String {
    "file_watch".into()
}

fn default_debounce_ms() -> u64 {
    200
}

fn default_true() -> bool {
    true
}

fn default_trust_fraction_below_autonomous() -> f64 {
    0.005
}

fn default_burn_rate_days_runway() -> u32 {
    7
}

fn default_counterfactual_fraction() -> f64 {
    0.001
}

fn default_max_mutations_per_trace() -> usize {
    5
}

fn default_consensus_min_agreement() -> usize {
    2
}

fn default_consensus_fraction() -> f64 {
    0.1
}

fn default_corpus_dir() -> PathBuf {
    PathBuf::from("tests/regression-corpus")
}
