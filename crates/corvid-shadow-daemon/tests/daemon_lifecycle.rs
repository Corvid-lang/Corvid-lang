use corvid_shadow_daemon::config::{
    AlertsConfig, DaemonConfig, DaemonSection, EnrollmentConfig, ExportConfig, SubscribeConfig,
};
use corvid_shadow_daemon::daemon::start_daemon;
use corvid_shadow_daemon::subscribe::{FileWatchSubscription, TraceSubscription};
use tokio::time::{timeout, Duration};

fn valid_config(dir: &std::path::Path) -> DaemonConfig {
    let program = dir.join("program.cor");
    std::fs::write(&program, "agent main() -> Int:\n    return 1\n").unwrap();
    DaemonConfig {
        daemon: DaemonSection {
            trace_dir: dir.join("trace"),
            ir_path: program,
            execution_tier: "interpreter".into(),
            max_concurrent_replays: 2,
            alert_log: dir.join("alerts.jsonl"),
        },
        subscribe: SubscribeConfig::default(),
        alerts: AlertsConfig::default(),
        enrollment: EnrollmentConfig::default(),
        exports: ExportConfig::default(),
    }
}

#[tokio::test]
async fn daemon_starts_with_valid_config_and_emits_ready_signal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("trace")).unwrap();
    let config_path = dir.path().join("shadow.toml");
    let config = valid_config(dir.path());
    std::fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();

    let handle = start_daemon(&config_path).await.unwrap();
    let status = handle.status().await;
    assert!(status.running);
    handle.shutdown().await;
    handle.wait().await;
}

#[tokio::test]
async fn daemon_exits_cleanly_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("trace")).unwrap();
    let config_path = dir.path().join("shadow.toml");
    let config = valid_config(dir.path());
    std::fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();

    let handle = start_daemon(&config_path).await.unwrap();
    handle.shutdown().await;
    handle.wait().await;
    assert!(!handle.status().await.running);
}

#[tokio::test]
async fn daemon_rejects_invalid_config_with_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("shadow.toml");
    std::fs::write(&config_path, "[daemon]\ntrace_dir = 123\n").unwrap();
    let err = match start_daemon(&config_path).await {
        Ok(_) => panic!("expected invalid config to fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("failed to parse daemon config"));
}

#[tokio::test]
async fn daemon_picks_up_new_trace_files_as_they_appear() {
    let dir = tempfile::tempdir().unwrap();
    let trace_dir = dir.path().join("trace");
    std::fs::create_dir_all(&trace_dir).unwrap();
    let mut subscription = FileWatchSubscription::watch(&trace_dir, 50).unwrap();
    let path = trace_dir.join("new.jsonl");
    std::fs::write(&path, "{\"kind\":\"schema_header\"}\n").unwrap();
    let seen = subscription.next().await.unwrap();
    assert_eq!(seen, path);
}

#[tokio::test]
async fn daemon_ignores_partial_writes() {
    let dir = tempfile::tempdir().unwrap();
    let trace_dir = dir.path().join("trace");
    std::fs::create_dir_all(&trace_dir).unwrap();
    let mut subscription = FileWatchSubscription::watch(&trace_dir, 500).unwrap();
    let path = trace_dir.join("partial.jsonl");
    std::fs::write(&path, "{\"kind\":\"schema_header\"}\n").unwrap();
    let first = subscription.next().await.unwrap();
    std::fs::write(&path, "{\"kind\":\"schema_header\"}\n{\"kind\":\"run_started\"}\n").unwrap();
    assert_eq!(first, path);
    assert!(timeout(Duration::from_millis(100), subscription.next()).await.is_err());
}
