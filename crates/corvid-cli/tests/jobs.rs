use std::path::PathBuf;
use std::process::Command;

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

#[test]
fn jobs_enqueue_and_run_one_persist_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let enqueue = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "daily_brief",
            "--payload",
            "{\"user\":\"u1\"}",
            "--input-schema",
            "DailyBriefInput",
            "--max-retries",
            "2",
            "--budget-usd",
            "0.50",
            "--effect-summary",
            "llm+db",
            "--replay-key",
            "replay:daily:u1",
        ])
        .output()
        .expect("run jobs enqueue");
    assert!(
        enqueue.status.success(),
        "enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&enqueue.stdout),
        String::from_utf8_lossy(&enqueue.stderr)
    );
    let stdout = String::from_utf8_lossy(&enqueue.stdout);
    assert!(stdout.contains("corvid jobs enqueue"), "{stdout}");
    assert!(stdout.contains("task: daily_brief"), "{stdout}");
    assert!(stdout.contains("input_schema: DailyBriefInput"), "{stdout}");
    assert!(stdout.contains("status: pending"), "{stdout}");
    assert!(stdout.contains("effect_summary: llm+db"), "{stdout}");

    let run = Command::new(corvid_bin())
        .args([
            "jobs",
            "run-one",
            "--state",
            state.to_str().unwrap(),
            "--output-kind",
            "DailyBriefOutput",
            "--output-fingerprint",
            "sha256:daily-output",
        ])
        .output()
        .expect("run jobs run-one");
    assert!(
        run.status.success(),
        "run-one failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("corvid jobs run-one"), "{stdout}");
    assert!(stdout.contains("task: daily_brief"), "{stdout}");
    assert!(stdout.contains("status: succeeded"), "{stdout}");
    assert!(stdout.contains("attempts: 1"), "{stdout}");
    assert!(stdout.contains("output_kind: DailyBriefOutput"), "{stdout}");
    assert!(stdout.contains("output_fingerprint: sha256:daily-output"), "{stdout}");
}

#[test]
fn jobs_dlq_inspects_dead_lettered_jobs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let enqueue = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "send_email",
            "--payload",
            "{\"draft\":\"d1\"}",
            "--max-retries",
            "0",
            "--replay-key",
            "replay:email:d1",
        ])
        .output()
        .expect("run jobs enqueue");
    assert!(
        enqueue.status.success(),
        "enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&enqueue.stdout),
        String::from_utf8_lossy(&enqueue.stderr)
    );

    let failed = Command::new(corvid_bin())
        .args([
            "jobs",
            "run-one",
            "--state",
            state.to_str().unwrap(),
            "--fail-kind",
            "provider_timeout",
            "--fail-fingerprint",
            "sha256:failure",
            "--retry-base-ms",
            "0",
        ])
        .output()
        .expect("run jobs failed attempt");
    assert!(
        failed.status.success(),
        "failed run failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&failed.stdout),
        String::from_utf8_lossy(&failed.stderr)
    );
    let stdout = String::from_utf8_lossy(&failed.stdout);
    assert!(stdout.contains("status: dead_lettered"), "{stdout}");
    assert!(stdout.contains("failure_fingerprint: sha256:failure"), "{stdout}");

    let dlq = Command::new(corvid_bin())
        .args(["jobs", "dlq", "--state", state.to_str().unwrap()])
        .output()
        .expect("run jobs dlq");
    assert!(
        dlq.status.success(),
        "dlq failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&dlq.stdout),
        String::from_utf8_lossy(&dlq.stderr)
    );
    let stdout = String::from_utf8_lossy(&dlq.stdout);
    assert!(stdout.contains("corvid jobs dlq"), "{stdout}");
    assert!(stdout.contains("dead_lettered_count: 1"), "{stdout}");
    assert!(stdout.contains("task:send_email"), "{stdout}");
    assert!(stdout.contains("failure_kind:provider_timeout"), "{stdout}");
    assert!(stdout.contains("failure_fingerprint:sha256:failure"), "{stdout}");
    assert!(stdout.contains("replay_key:replay:email:d1"), "{stdout}");
}
