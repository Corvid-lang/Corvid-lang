pub mod alerts;
pub mod config;
pub mod daemon;
pub mod enrollment;
pub mod exports;
pub mod replay_pool;
pub mod subscribe;

pub use alerts::{Alert, AlertKind, AlertSeverity, AlertSink, ShadowAlertContext};
pub use config::{
    load_config, AlertLogConfig, AlertsConfig, ConsensusAlertConfig, CounterfactualAlertConfig,
    DaemonConfig, DaemonSection, DimensionAlertBudgetConfig, DimensionAlertConfig,
    DimensionAlertLatencyConfig, DimensionAlertTrustConfig, EnrollmentConfig, ExportConfig,
    ExportOtelConfig, ExportPrometheusConfig, SubscribeConfig,
};
pub use daemon::{
    ack_trace, dump_alerts, ShadowDaemon, ShadowDaemonHandle, ShadowDaemonStatus, start_daemon,
};
pub use enrollment::{AckMetadata, EnrollmentAction, EnrollmentManager};
pub use exports::{ExportSink, OtelExporter, PrometheusExporter};
pub use replay_pool::{
    approval_label_for_tool, parse_program_source, AgentInvariantInfo, DangerousToolSpec,
    DimensionSnapshot, InterpreterShadowExecutor, MutationSpec, ProvenanceSnapshot, ReplayPool,
    ShadowExecutionMode, ShadowExecutorError, ShadowReplayExecutor, ShadowReplayOutcome,
    TrustTier,
};
pub use subscribe::{FileWatchSubscription, TraceSubscription};
