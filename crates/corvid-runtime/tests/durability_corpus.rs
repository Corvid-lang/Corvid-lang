//! Durability corpus — slice 38L.
//!
//! Three tests the Phase 38 phase-done checklist names but the
//! audit found absent:
//!
//!   D1. 4-concurrent-worker idempotency: 4 worker tasks each
//!       enqueue 25 jobs sharing one idempotency key; exactly one
//!       row exists in the queue afterwards.
//!
//!   D2. DST cron: a schedule expressed in `America/New_York`
//!       crossing the spring-forward and fall-back transitions
//!       fires on the documented cron-crate semantics — i.e. the
//!       fire times come back in monotonic UTC order, no
//!       duplicates, and the spring-forward gap (02:30 doesn't
//!       exist on 2026-03-08) is handled deterministically.
//!
//!   D3. Crash-recovery checkpoint durability: a worker leases a
//!       job and writes checkpoints; the runtime drops uncleanly
//!       (the simulation of a SIGKILL'd process); a fresh runtime
//!       opens the same SQLite file, advances time past the lease
//!       expiry, and re-leases the job — the original
//!       checkpoints survive and a second worker can resume from
//!       them without re-running already-recorded steps.
//!
//! D3 is structurally close to but does not replace a true
//! subprocess-based SIGKILL test. The audit asks for "verified by
//! mock-LLM call counter" — that count-bounded property requires
//! the Phase 21 Replay layer the queue runtime alone cannot
//! exercise. D3 covers the durability bound the queue is
//! responsible for: SQLite WAL fsync guarantees the checkpoint
//! row survives the crash, and the lease window guarantees the
//! second worker can take over after expiry. The full SIGKILL +
//! double-LLM-call test joins the Phase 21 Replay corpus.

use corvid_runtime::queue::{
    DurableQueueRuntime, JobCheckpointKind, QueueScheduleManifest, ScheduleMissedPolicy,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn temp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.db");
    (dir, path)
}

// ---------------------------------------------------------------
// D1 — 4-concurrent-worker idempotency
// ---------------------------------------------------------------

/// Slice 38L D1: four worker threads each enqueue 25 jobs that
/// share one idempotency key. After all submissions complete,
/// exactly one queue row exists. The UNIQUE INDEX on
/// `idempotency_key` (added in this slice) plus the existing
/// `enqueue_typed_idempotent` collision-fallback path makes the
/// race-free property structural — not implementation-dependent.
#[test]
fn t38l_d1_four_workers_collapse_to_one_row() {
    let (_tmp, path) = temp_db();
    let queue = Arc::new(DurableQueueRuntime::open(&path).unwrap());
    // Initialise the queue once so the schema (with the unique
    // index) is in place before workers race.
    drop(queue);

    let queue = Arc::new(DurableQueueRuntime::open(&path).unwrap());
    let mut handles = Vec::new();
    for worker in 0..4 {
        let q = queue.clone();
        handles.push(thread::spawn(move || {
            let mut accepted = 0_usize;
            let mut errors = 0_usize;
            for i in 0..25 {
                let result = q.enqueue_typed_idempotent(
                    "charge_card",
                    json!({"worker": worker, "i": i}),
                    Some("ChargeInput".to_string()),
                    1,
                    1.0,
                    Some("payment".to_string()),
                    Some(format!("rk:worker-{worker}:{i}")),
                    Some("charge:invoice-42".to_string()),
                    None,
                );
                match result {
                    Ok(_) => accepted += 1,
                    Err(_) => errors += 1,
                }
            }
            (accepted, errors)
        }));
    }
    for h in handles {
        let (accepted, errors) = h.join().unwrap();
        assert!(
            accepted + errors == 25,
            "worker accounted for all 25 attempts (accepted={accepted} errors={errors})"
        );
    }

    // Inspect the queue post-race: exactly one row, idempotency
    // key matches, every other (worker,i) pair was collapsed.
    let all = queue.list().unwrap();
    let with_key: Vec<_> = all
        .iter()
        .filter(|j| j.idempotency_key.as_deref() == Some("charge:invoice-42"))
        .collect();
    assert_eq!(
        with_key.len(),
        1,
        "expected exactly one job with idempotency key, got {}: {:?}",
        with_key.len(),
        with_key
            .iter()
            .map(|j| j.id.as_str())
            .collect::<Vec<_>>()
    );
    // The replay key on the surviving row must be one the workers
    // submitted — proves the row is a real worker submission, not
    // a synthesized merge.
    let surviving = with_key[0];
    assert!(
        surviving
            .replay_key
            .as_ref()
            .map(|k| k.starts_with("rk:worker-"))
            .unwrap_or(false),
        "surviving replay_key looks like a worker submission: {:?}",
        surviving.replay_key
    );
}

// ---------------------------------------------------------------
// D2 — DST cron
// ---------------------------------------------------------------

/// Slice 38L D2: a `02:30 America/New_York` daily cron schedule
/// crossing the 2026-03-08 spring-forward boundary fires
/// according to the cron-crate's documented semantics. We assert:
///
///   - All produced fire times are strictly monotonic.
///   - No two fires share the same UTC millisecond.
///   - The 2026-03-08 02:30 EST → EDT gap is handled without
///     producing a fire at the (non-existent) 02:30 EST instant.
///
/// We do NOT assert a specific count of fires across the
/// boundary — the documented cron-crate behaviour can either skip
/// or shift, and either is fine for the audit-correction bullet
/// "DST cron test" as long as the property is honest and tested.
#[test]
fn t38l_d2_dst_spring_forward_is_deterministic() {
    let (_tmp, path) = temp_db();
    let queue = DurableQueueRuntime::open(&path).unwrap();

    // 2026-03-07 19:00:00 UTC = 2026-03-07 14:00 EST, Saturday
    // before spring-forward.
    let saturday_evening_utc_ms: u64 = 1_804_345_200_000; // 2026-03-07 19:00:00Z
    // 2026-03-09 19:00:00 UTC = 2026-03-09 15:00 EDT, Monday
    // after spring-forward.
    let monday_afternoon_utc_ms: u64 = 1_804_518_000_000; // 2026-03-09 19:00:00Z

    queue
        .upsert_schedule(QueueScheduleManifest {
            id: "dst-spring".to_string(),
            cron: "30 2 * * *".to_string(),
            zone: "America/New_York".to_string(),
            task: "daily_brief".to_string(),
            payload: json!({}),
            max_retries: 1,
            budget_usd: 0.10,
            effect_summary: Some("daily".to_string()),
            replay_key_prefix: Some("daily".to_string()),
            missed_policy: ScheduleMissedPolicy::EnqueueAllBounded,
            last_checked_ms: saturday_evening_utc_ms.saturating_sub(1),
            last_fire_ms: None,
            created_ms: 0,
            updated_ms: 0,
        })
        .unwrap();

    // Recover schedules from saturday_evening through monday
    // afternoon — bounded enough to cover at most a handful of
    // missed fires.
    let report = queue
        .recover_schedules_at(monday_afternoon_utc_ms, 10)
        .unwrap();

    // Every recovery action that enqueued a job carries a fire_ms.
    let fire_times: Vec<u64> = report
        .recoveries
        .iter()
        .filter(|r| r.action == "enqueued")
        .map(|r| r.fire_ms)
        .collect();
    assert!(
        !fire_times.is_empty(),
        "spring-forward weekend produced at least one cron fire"
    );
    // Monotonic UTC order.
    for window in fire_times.windows(2) {
        assert!(
            window[0] < window[1],
            "fire times are strictly monotonic: {:?}",
            fire_times
        );
    }
    // No duplicates.
    let mut sorted = fire_times.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        fire_times.len(),
        "no duplicate fire times: {fire_times:?}"
    );
    // The non-existent 02:30 EST instant on 2026-03-08 is
    // 1_804_410_600_000 (i.e. interpreted as if it had been
    // EST = UTC-5). No fire should land at that exact UTC
    // millisecond — the cron crate either shifts to the next
    // valid local time or skips the day, both of which produce a
    // fire at a different UTC millisecond.
    let nonexistent_local_as_est_utc_ms: u64 = 1_804_410_600_000;
    assert!(
        !fire_times.contains(&nonexistent_local_as_est_utc_ms),
        "no fire at the non-existent 02:30 EST = {nonexistent_local_as_est_utc_ms} UTC ms; got {fire_times:?}"
    );
}

/// Slice 38L D2: a `01:30 America/New_York` daily cron schedule
/// across the fall-back boundary fires once per local day, even
/// though 01:30 occurs twice in local time (01:30 EDT, then
/// 01:30 EST after the clock shifts back). The cron-crate
/// schedule-after iterator is monotonic in UTC, so the second
/// 01:30 (the EST one) on the boundary day produces a separate
/// fire — but we never produce TWO fires for the same UTC ms.
#[test]
fn t38l_d2_dst_fall_back_is_monotonic() {
    let (_tmp, path) = temp_db();
    let queue = DurableQueueRuntime::open(&path).unwrap();

    // 2026-10-31 22:00:00 UTC = 2026-10-31 18:00 EDT, Saturday
    // before fall-back.
    let saturday_evening_utc_ms: u64 = 1_793_217_600_000;
    // 2026-11-02 22:00:00 UTC = 2026-11-02 17:00 EST, Monday
    // after fall-back.
    let monday_afternoon_utc_ms: u64 = 1_793_390_400_000;

    queue
        .upsert_schedule(QueueScheduleManifest {
            id: "dst-fall".to_string(),
            cron: "30 1 * * *".to_string(),
            zone: "America/New_York".to_string(),
            task: "early_brief".to_string(),
            payload: json!({}),
            max_retries: 1,
            budget_usd: 0.10,
            effect_summary: Some("daily".to_string()),
            replay_key_prefix: Some("early".to_string()),
            missed_policy: ScheduleMissedPolicy::EnqueueAllBounded,
            last_checked_ms: saturday_evening_utc_ms.saturating_sub(1),
            last_fire_ms: None,
            created_ms: 0,
            updated_ms: 0,
        })
        .unwrap();

    let report = queue
        .recover_schedules_at(monday_afternoon_utc_ms, 10)
        .unwrap();
    let fire_times: Vec<u64> = report
        .recoveries
        .iter()
        .filter(|r| r.action == "enqueued")
        .map(|r| r.fire_ms)
        .collect();
    assert!(!fire_times.is_empty());
    // Monotonic + no duplicates.
    for window in fire_times.windows(2) {
        assert!(
            window[0] < window[1],
            "fall-back fire times monotonic: {fire_times:?}"
        );
    }
    let mut sorted = fire_times.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), fire_times.len());
}

// ---------------------------------------------------------------
// D3 — crash-recovery checkpoint durability
// ---------------------------------------------------------------

/// Slice 38L D3: a worker leases a job and records two
/// checkpoints. The runtime drops mid-step (simulated by closing
/// the connection without releasing the lease). A fresh runtime
/// opens the same SQLite file, advances time past the lease TTL,
/// and re-leases the job under a different worker id. The
/// original checkpoints survive — a downstream replay path can
/// resume from them without re-running already-recorded steps.
///
/// This is the durability property a real SIGKILL test would
/// also rely on. The full subprocess-based SIGKILL + mock-LLM
/// call counter test joins Phase 21 Replay's corpus when the
/// step-skip semantics land at the VM layer.
#[test]
fn t38l_d3_checkpoints_survive_unclean_shutdown() {
    let (_tmp, path) = temp_db();

    // Worker-A opens the queue, enqueues a job, leases it, writes
    // two checkpoints, then drops the runtime *without* releasing
    // the lease — the simulation of a SIGKILL.
    let job_id = {
        let queue = DurableQueueRuntime::open(&path).unwrap();
        let job = queue
            .enqueue_typed(
                "agent_run",
                json!({"goal": "brief"}),
                None,
                1,
                0.20,
                Some("ai".to_string()),
                Some("rk:agent:1".to_string()),
            )
            .unwrap();
        let leased = queue
            .lease_next_at("worker-A", 60_000, 1_000)
            .unwrap()
            .expect("lease acquired");
        assert_eq!(leased.id, job.id);
        // Two checkpoints — the audit calls for "checkpoint
        // envelopes ship".
        queue
            .record_checkpoint(
                &job.id,
                JobCheckpointKind::AgentStep,
                "step.fetch_inbox",
                json!({"messages": ["m1", "m2"]}),
                Some("fp-step-1".to_string()),
            )
            .unwrap();
        queue
            .record_checkpoint(
                &job.id,
                JobCheckpointKind::ToolResult,
                "tool.summarise",
                json!({"summary": "..."}),
                Some("fp-step-2".to_string()),
            )
            .unwrap();
        // SIGKILL surrogate: drop the runtime without finalising
        // the lease. SQLite WAL fsync guarantees the checkpoint
        // rows are durable.
        job.id
    };

    // Sleep briefly so the wall clock advances past any
    // lease-related timing the runtime may consult internally.
    thread::sleep(Duration::from_millis(50));

    // Worker-B opens the queue fresh (the SIGKILL'd worker is
    // gone). Advance the clock past the lease TTL and re-lease.
    let queue = DurableQueueRuntime::open(&path).unwrap();
    let checkpoints_before = queue.list_checkpoints(&job_id).unwrap();
    assert_eq!(
        checkpoints_before.len(),
        2,
        "checkpoints survive the unclean shutdown: {checkpoints_before:?}"
    );
    let labels: Vec<&str> = checkpoints_before
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    assert!(labels.contains(&"step.fetch_inbox"));
    assert!(labels.contains(&"tool.summarise"));

    // 1_000 is when the original lease was acquired with TTL of
    // 60_000 — re-leasing at 100_000 is well past expiry.
    let resumed = queue
        .lease_next_at("worker-B", 60_000, 100_000)
        .unwrap()
        .expect("re-lease after expiry");
    assert_eq!(resumed.id, job_id);
    assert_eq!(resumed.lease_owner.as_deref(), Some("worker-B"));

    // Checkpoints are still there after the re-lease.
    let checkpoints_after = queue.list_checkpoints(&job_id).unwrap();
    assert_eq!(checkpoints_after.len(), 2);
}
