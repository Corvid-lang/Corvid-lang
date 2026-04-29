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
    assert!(
        stdout.contains("recovery: schedule:daily_brief"),
        "{stdout}"
    );

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
    assert!(
        stdout.contains("replay_key: schedule:daily_brief:"),
        "{stdout}"
    );
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
    assert!(
        stdout.contains("limit: scope:task:email max_leased:2"),
        "{stdout}"
    );
}

#[test]
fn jobs_idempotency_key_collapses_duplicate_enqueue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    for payload in [
        "{\"invoice\":\"i1\"}",
        "{\"invoice\":\"i1\",\"changed\":true}",
    ] {
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
fn jobs_loop_limits_stop_over_budget_agent_jobs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let enqueue = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "daily_brief_agent",
            "--payload",
            "{\"user\":\"u1\"}",
            "--budget-usd",
            "0.20",
            "--effect-summary",
            "llm+tools",
            "--replay-key",
            "replay:brief:u1",
        ])
        .output()
        .expect("run jobs enqueue");
    assert!(
        enqueue.status.success(),
        "enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&enqueue.stdout),
        String::from_utf8_lossy(&enqueue.stderr)
    );

    let limits = Command::new(corvid_bin())
        .args([
            "jobs",
            "loop",
            "limits",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
            "--max-steps",
            "2",
            "--max-wall-ms",
            "1000",
            "--max-spend-usd",
            "0.20",
            "--max-tool-calls",
            "1",
        ])
        .output()
        .expect("set loop limits");
    assert!(
        limits.status.success(),
        "limits failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&limits.stdout),
        String::from_utf8_lossy(&limits.stderr)
    );
    let stdout = String::from_utf8_lossy(&limits.stdout);
    assert!(stdout.contains("corvid jobs loop limits"), "{stdout}");
    assert!(stdout.contains("max_steps: 2"), "{stdout}");
    assert!(stdout.contains("max_spend_usd: 0.200000"), "{stdout}");

    let within = Command::new(corvid_bin())
        .args([
            "jobs",
            "loop",
            "record",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
            "--steps",
            "1",
            "--wall-ms",
            "300",
            "--spend-usd",
            "0.05",
            "--tool-calls",
            "1",
            "--actor",
            "worker-a",
        ])
        .output()
        .expect("record loop usage");
    assert!(
        within.status.success(),
        "record within failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&within.stdout),
        String::from_utf8_lossy(&within.stderr)
    );
    let stdout = String::from_utf8_lossy(&within.stdout);
    assert!(stdout.contains("violated_bound_count: 0"), "{stdout}");
    assert!(stdout.contains("status: pending"), "{stdout}");

    let exceeded = Command::new(corvid_bin())
        .args([
            "jobs",
            "loop",
            "record",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
            "--steps",
            "2",
            "--wall-ms",
            "800",
            "--spend-usd",
            "0.16",
            "--tool-calls",
            "1",
            "--actor",
            "worker-a",
        ])
        .output()
        .expect("record over-budget usage");
    assert!(
        exceeded.status.success(),
        "record exceeded failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&exceeded.stdout),
        String::from_utf8_lossy(&exceeded.stderr)
    );
    let stdout = String::from_utf8_lossy(&exceeded.stdout);
    assert!(stdout.contains("violated_bound_count: 4"), "{stdout}");
    assert!(stdout.contains("violated_bound: max_steps:3>2"), "{stdout}");
    assert!(stdout.contains("violated_bound: max_wall_ms:1100>1000"), "{stdout}");
    assert!(stdout.contains("violated_bound: max_spend_usd:0.210000>0.200000"), "{stdout}");
    assert!(stdout.contains("violated_bound: max_tool_calls:2>1"), "{stdout}");
    assert!(stdout.contains("status: loop_budget_exceeded"), "{stdout}");
    assert!(stdout.contains("failure_kind: loop_bound_exceeded"), "{stdout}");

    let audit = Command::new(corvid_bin())
        .args([
            "jobs",
            "approval",
            "audit",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
        ])
        .output()
        .expect("audit loop violation");
    assert!(
        audit.status.success(),
        "audit failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&audit.stdout),
        String::from_utf8_lossy(&audit.stderr)
    );
    let stdout = String::from_utf8_lossy(&audit.stdout);
    assert!(stdout.contains("event_kind: loop_bound_exceeded"), "{stdout}");
    assert!(stdout.contains("status_after: loop_budget_exceeded"), "{stdout}");

    let conn = Connection::open(&state).expect("open jobs db");
    let status: String = conn
        .query_row("select status from queue_jobs where id = 'job_1'", [], |row| row.get(0))
        .expect("read status");
    assert_eq!(status, "loop_budget_exceeded");
}

#[test]
fn jobs_loop_stall_policy_escalates_with_audit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let enqueue = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "daily_brief_agent",
            "--payload",
            "{\"user\":\"u1\"}",
        ])
        .output()
        .expect("run jobs enqueue");
    assert!(
        enqueue.status.success(),
        "enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&enqueue.stdout),
        String::from_utf8_lossy(&enqueue.stderr)
    );

    let heartbeat = Command::new(corvid_bin())
        .args([
            "jobs",
            "loop",
            "heartbeat",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
            "--actor",
            "worker-a",
            "--message",
            "planning",
        ])
        .output()
        .expect("record heartbeat");
    assert!(
        heartbeat.status.success(),
        "heartbeat failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&heartbeat.stdout),
        String::from_utf8_lossy(&heartbeat.stderr)
    );
    let stdout = String::from_utf8_lossy(&heartbeat.stdout);
    assert!(stdout.contains("corvid jobs loop heartbeat"), "{stdout}");
    assert!(stdout.contains("actor: worker-a"), "{stdout}");

    let policy = Command::new(corvid_bin())
        .args([
            "jobs",
            "loop",
            "stall-policy",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
            "--stall-after-ms",
            "1",
            "--action",
            "escalate",
        ])
        .output()
        .expect("set stall policy");
    assert!(
        policy.status.success(),
        "stall policy failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&policy.stdout),
        String::from_utf8_lossy(&policy.stderr)
    );

    std::thread::sleep(std::time::Duration::from_millis(5));
    let check = Command::new(corvid_bin())
        .args([
            "jobs",
            "loop",
            "check-stall",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
            "--actor",
            "watchdog",
        ])
        .output()
        .expect("check stall");
    assert!(
        check.status.success(),
        "check stall failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(stdout.contains("stalled: true"), "{stdout}");
    assert!(stdout.contains("action_taken: escalate"), "{stdout}");
    assert!(stdout.contains("status: loop_stall_escalated"), "{stdout}");
    assert!(stdout.contains("failure_kind: loop_stalled"), "{stdout}");

    let audit = Command::new(corvid_bin())
        .args([
            "jobs",
            "approval",
            "audit",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
        ])
        .output()
        .expect("audit stall");
    assert!(
        audit.status.success(),
        "audit failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&audit.stdout),
        String::from_utf8_lossy(&audit.stderr)
    );
    let stdout = String::from_utf8_lossy(&audit.stdout);
    assert!(stdout.contains("event_kind: loop_stalled_escalate"), "{stdout}");
    assert!(stdout.contains("status_after: loop_stall_escalated"), "{stdout}");
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

    let resume = Command::new(corvid_bin())
        .args([
            "jobs",
            "checkpoint",
            "resume",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
        ])
        .output()
        .expect("run checkpoint resume");
    assert!(
        resume.status.success(),
        "checkpoint resume failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&resume.stdout),
        String::from_utf8_lossy(&resume.stderr)
    );
    let stdout = String::from_utf8_lossy(&resume.stdout);
    assert!(stdout.contains("checkpoint_count: 3"), "{stdout}");
    assert!(stdout.contains("next_sequence: 4"), "{stdout}");
    assert!(stdout.contains("last_kind: partial_output"), "{stdout}");
    assert!(stdout.contains("last_label: draft"), "{stdout}");
}

#[test]
fn jobs_wait_approval_pauses_and_lists_approval_wait_jobs() {
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
            "--effect-summary",
            "email:write approve:send",
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

    let wait = Command::new(corvid_bin())
        .args([
            "jobs",
            "wait-approval",
            "--state",
            state.to_str().unwrap(),
            "--worker-id",
            "worker-a",
            "--approval-id",
            "approval:send:d1",
            "--approval-expires-ms",
            "4102444800000",
            "--approval-reason",
            "send external email draft d1",
        ])
        .output()
        .expect("run approval wait");
    assert!(
        wait.status.success(),
        "approval wait failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&wait.stdout),
        String::from_utf8_lossy(&wait.stderr)
    );
    let stdout = String::from_utf8_lossy(&wait.stdout);
    assert!(stdout.contains("corvid jobs wait-approval"), "{stdout}");
    assert!(stdout.contains("status: approval_wait"), "{stdout}");
    assert!(stdout.contains("approval_id: approval:send:d1"), "{stdout}");
    assert!(
        stdout.contains("approval_expires_ms: 4102444800000"),
        "{stdout}"
    );
    assert!(
        stdout.contains("approval_reason: send external email draft d1"),
        "{stdout}"
    );
    assert!(stdout.contains("lease_owner: "), "{stdout}");

    let run = Command::new(corvid_bin())
        .args(["jobs", "run-one", "--state", state.to_str().unwrap()])
        .output()
        .expect("run jobs run-one");
    assert!(
        run.status.success(),
        "run-one failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("job: none"), "{stdout}");

    let approvals = Command::new(corvid_bin())
        .args(["jobs", "approvals", "--state", state.to_str().unwrap()])
        .output()
        .expect("list approval waits");
    assert!(
        approvals.status.success(),
        "approvals failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&approvals.stdout),
        String::from_utf8_lossy(&approvals.stderr)
    );
    let stdout = String::from_utf8_lossy(&approvals.stdout);
    assert!(stdout.contains("approval_wait_count: 1"), "{stdout}");
    assert!(stdout.contains("task: send_email"), "{stdout}");

    let approve = Command::new(corvid_bin())
        .args([
            "jobs",
            "approval",
            "decide",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
            "--approval-id",
            "approval:send:d1",
            "--decision",
            "approve",
            "--actor",
            "reviewer:u1",
            "--reason",
            "approved redacted email",
        ])
        .output()
        .expect("approve job");
    assert!(
        approve.status.success(),
        "approve failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&approve.stdout),
        String::from_utf8_lossy(&approve.stderr)
    );
    let stdout = String::from_utf8_lossy(&approve.stdout);
    assert!(stdout.contains("corvid jobs approval decide"), "{stdout}");
    assert!(stdout.contains("decision: approve"), "{stdout}");
    assert!(stdout.contains("status: pending"), "{stdout}");

    let audit = Command::new(corvid_bin())
        .args([
            "jobs",
            "approval",
            "audit",
            "--state",
            state.to_str().unwrap(),
            "--job",
            "job_1",
        ])
        .output()
        .expect("audit approval decision");
    assert!(
        audit.status.success(),
        "audit failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&audit.stdout),
        String::from_utf8_lossy(&audit.stderr)
    );
    let stdout = String::from_utf8_lossy(&audit.stdout);
    assert!(stdout.contains("audit_event_count: 1"), "{stdout}");
    assert!(stdout.contains("event_kind: approval_approve"), "{stdout}");
    assert!(stdout.contains("actor: reviewer:u1"), "{stdout}");
    assert!(stdout.contains("status_before: approval_wait"), "{stdout}");
    assert!(stdout.contains("status_after: pending"), "{stdout}");
    assert!(
        stdout.contains("reason: approved redacted email"),
        "{stdout}"
    );

    let resumed = Command::new(corvid_bin())
        .args([
            "jobs",
            "run-one",
            "--state",
            state.to_str().unwrap(),
            "--output-kind",
            "EmailSendResult",
            "--output-fingerprint",
            "sha256:redacted-email-send",
        ])
        .output()
        .expect("run approved job");
    assert!(
        resumed.status.success(),
        "run approved failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    let stdout = String::from_utf8_lossy(&resumed.stdout);
    assert!(stdout.contains("status: succeeded"), "{stdout}");

    let conn = Connection::open(&state).expect("open jobs db");
    let status: String = conn
        .query_row(
            "select status from queue_jobs where id = 'job_1'",
            [],
            |row| row.get(0),
        )
        .expect("read status");
    assert_eq!(status, "succeeded");
    let audit_count: i64 = conn
        .query_row("select count(*) from queue_job_audit_events", [], |row| {
            row.get(0)
        })
        .expect("count audit events");
    assert_eq!(audit_count, 1);
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
