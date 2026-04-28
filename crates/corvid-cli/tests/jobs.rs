use std::path::PathBuf;
use std::process::Command;

use rusqlite::Connection;

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
    assert!(
        stdout.contains("output_fingerprint: sha256:daily-output"),
        "{stdout}"
    );
}

#[test]
fn jobs_delay_persists_and_skips_until_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let delayed = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "scheduled_digest",
            "--payload",
            "{\"team\":\"eng\"}",
            "--delay-ms",
            "60000",
        ])
        .output()
        .expect("run delayed jobs enqueue");
    assert!(
        delayed.status.success(),
        "delayed enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&delayed.stdout),
        String::from_utf8_lossy(&delayed.stderr)
    );
    let stdout = String::from_utf8_lossy(&delayed.stdout);
    assert!(stdout.contains("task: scheduled_digest"), "{stdout}");
    assert!(stdout.contains("next_run_ms: "), "{stdout}");

    let skipped = Command::new(corvid_bin())
        .args(["jobs", "run-one", "--state", state.to_str().unwrap()])
        .output()
        .expect("run jobs run-one before delay");
    assert!(
        skipped.status.success(),
        "run-one failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&skipped.stdout),
        String::from_utf8_lossy(&skipped.stderr)
    );
    let stdout = String::from_utf8_lossy(&skipped.stdout);
    assert!(stdout.contains("job: none"), "{stdout}");

    let immediate = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "immediate_digest",
        ])
        .output()
        .expect("run immediate jobs enqueue");
    assert!(
        immediate.status.success(),
        "immediate enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&immediate.stdout),
        String::from_utf8_lossy(&immediate.stderr)
    );

    let run = Command::new(corvid_bin())
        .args(["jobs", "run-one", "--state", state.to_str().unwrap()])
        .output()
        .expect("run jobs run-one after immediate enqueue");
    assert!(
        run.status.success(),
        "run-one failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("task: immediate_digest"), "{stdout}");
    assert!(!stdout.contains("task: scheduled_digest"), "{stdout}");
}

#[test]
fn jobs_schedule_recovers_missed_fire_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let add = Command::new(corvid_bin())
        .args([
            "jobs",
            "schedule",
            "add",
            "--state",
            state.to_str().unwrap(),
            "--id",
            "daily_brief",
            "--cron",
            "* * * * *",
            "--zone",
            "UTC",
            "--task",
            "daily_brief",
            "--payload",
            "{\"user\":\"u1\"}",
            "--effect-summary",
            "llm+email",
            "--replay-key-prefix",
            "schedule:daily_brief",
        ])
        .output()
        .expect("run jobs schedule add");
    assert!(
        add.status.success(),
        "schedule add failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr)
    );

    let conn = Connection::open(&state).expect("open schedule db");
    let now = corvid_runtime::tracing::now_ms();
    conn.execute(
        "update queue_schedules set last_checked_ms = ?1, last_fire_ms = null where id = 'daily_brief'",
        [now.saturating_sub(5 * 60_000) as i64],
    )
    .expect("rewind schedule cursor");
    drop(conn);

    let recover = Command::new(corvid_bin())
        .args([
            "jobs",
            "schedule",
            "recover",
            "--state",
            state.to_str().unwrap(),
            "--max-missed-per-schedule",
            "8",
        ])
        .output()
        .expect("run jobs schedule recover");
    assert!(
        recover.status.success(),
        "schedule recover failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&recover.stdout),
        String::from_utf8_lossy(&recover.stderr)
    );
    let stdout = String::from_utf8_lossy(&recover.stdout);
    assert!(stdout.contains("corvid jobs schedule recover"), "{stdout}");
    assert!(stdout.contains("scanned: 1"), "{stdout}");
    assert!(stdout.contains("enqueued: 1"), "{stdout}");
    assert!(stdout.contains("recovery: schedule:daily_brief"), "{stdout}");

    let run = Command::new(corvid_bin())
        .args(["jobs", "run-one", "--state", state.to_str().unwrap()])
        .output()
        .expect("run recovered job");
    assert!(
        run.status.success(),
        "run-one failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("task: daily_brief"), "{stdout}");
    assert!(stdout.contains("replay_key: schedule:daily_brief:"), "{stdout}");
}

#[test]
fn jobs_limit_cli_persists_concurrency_limits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let set = Command::new(corvid_bin())
        .args([
            "jobs",
            "limit",
            "set",
            "--state",
            state.to_str().unwrap(),
            "--scope",
            "task",
            "--task",
            "email",
            "--max-leased",
            "2",
        ])
        .output()
        .expect("run jobs limit set");
    assert!(
        set.status.success(),
        "limit set failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&set.stdout),
        String::from_utf8_lossy(&set.stderr)
    );
    let stdout = String::from_utf8_lossy(&set.stdout);
    assert!(stdout.contains("corvid jobs limit set"), "{stdout}");
    assert!(stdout.contains("scope: task:email"), "{stdout}");
    assert!(stdout.contains("max_leased: 2"), "{stdout}");

    let list = Command::new(corvid_bin())
        .args(["jobs", "limit", "list", "--state", state.to_str().unwrap()])
        .output()
        .expect("run jobs limit list");
    assert!(
        list.status.success(),
        "limit list failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&list.stdout),
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("limit_count: 1"), "{stdout}");
    assert!(stdout.contains("limit: scope:task:email max_leased:2"), "{stdout}");
}

#[test]
fn jobs_idempotency_key_collapses_duplicate_enqueue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    for payload in ["{\"invoice\":\"i1\"}", "{\"invoice\":\"i1\",\"changed\":true}"] {
        let enqueue = Command::new(corvid_bin())
            .args([
                "jobs",
                "enqueue",
                "--state",
                state.to_str().unwrap(),
                "--task",
                "charge_card",
                "--payload",
                payload,
                "--idempotency-key",
                "charge:i1",
            ])
            .output()
            .expect("run idempotent enqueue");
        assert!(
            enqueue.status.success(),
            "enqueue failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&enqueue.stdout),
            String::from_utf8_lossy(&enqueue.stderr)
        );
        let stdout = String::from_utf8_lossy(&enqueue.stdout);
        assert!(stdout.contains("job: job_1"), "{stdout}");
        assert!(stdout.contains("idempotency_key: charge:i1"), "{stdout}");
    }

    let conn = Connection::open(&state).expect("open jobs db");
    let count: i64 = conn
        .query_row("select count(*) from queue_jobs", [], |row| row.get(0))
        .expect("count jobs");
    assert_eq!(count, 1);
}

#[test]
fn jobs_checkpoint_cli_records_agent_tool_and_partial_outputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let enqueue = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "agent_run",
            "--payload",
            "{\"goal\":\"brief\"}",
        ])
        .output()
        .expect("run jobs enqueue");
    assert!(
        enqueue.status.success(),
        "enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&enqueue.stdout),
        String::from_utf8_lossy(&enqueue.stderr)
    );

    for (kind, label, payload) in [
        ("agent-step", "plan", "{\"step\":1}"),
        ("tool-result", "gmail.search", "{\"result_count\":3}"),
        ("partial-output", "draft", "{\"chars\":120}"),
    ] {
        let checkpoint = Command::new(corvid_bin())
            .args([
                "jobs",
                "checkpoint",
                "add",
                "--state",
                state.to_str().unwrap(),
                "--job",
                "job_1",
                "--kind",
                kind,
                "--label",
                label,
                "--payload",
                payload,
                "--payload-fingerprint",
                "sha256:redacted",
            ])
            .output()
            .expect("run checkpoint add");
        assert!(
            checkpoint.status.success(),
            "checkpoint failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&checkpoint.stdout),
            String::from_utf8_lossy(&checkpoint.stderr)
        );
    }

    let list = Command::new(corvid_bin())
        .args([
            "jobs",
            "checkpoint",
            "list",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
        ])
        .output()
        .expect("run checkpoint list");
    assert!(
        list.status.success(),
        "checkpoint list failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&list.stdout),
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("checkpoint_count: 3"), "{stdout}");
    assert!(stdout.contains("kind: agent_step"), "{stdout}");
    assert!(stdout.contains("kind: tool_result"), "{stdout}");
    assert!(stdout.contains("kind: partial_output"), "{stdout}");
    assert!(stdout.contains("label: gmail.search"), "{stdout}");
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
    assert!(
        stdout.contains("failure_fingerprint: sha256:failure"),
        "{stdout}"
    );

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
    assert!(
        stdout.contains("failure_fingerprint:sha256:failure"),
        "{stdout}"
    );
    assert!(stdout.contains("replay_key:replay:email:d1"), "{stdout}");
}
