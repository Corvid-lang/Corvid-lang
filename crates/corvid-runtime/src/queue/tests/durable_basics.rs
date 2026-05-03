use super::*;

#[test]
fn queue_enqueue_and_cancel_preserve_metadata() {
    let queue = QueueRuntime::new();
    let job = queue
        .enqueue(
            "embed",
            serde_json::json!({"doc": "a"}),
            3,
            0.25,
            Some("llm+io".to_string()),
            Some("trace:1".to_string()),
        )
        .unwrap();

    assert_eq!(job.status, QueueJobStatus::Pending);
    assert_eq!(job.max_retries, 3);
    assert_eq!(job.effect_summary.as_deref(), Some("llm+io"));
    assert_eq!(
        queue.get(&job.id).unwrap().replay_key.as_deref(),
        Some("trace:1")
    );

    let canceled = queue.cancel(&job.id).unwrap();
    assert_eq!(canceled.status, QueueJobStatus::Canceled);
}

#[test]
fn durable_queue_persists_jobs_and_supports_cancel_and_list() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let job = queue
        .enqueue(
            "embed",
            serde_json::json!({"doc": "a"}),
            2,
            1.25,
            Some("llm+io".to_string()),
            Some("trace:2".to_string()),
        )
        .unwrap();

    let fetched = queue.get(&job.id).unwrap().unwrap();
    assert_eq!(fetched.task, "embed");
    assert_eq!(fetched.replay_key.as_deref(), Some("trace:2"));

    let listed = queue.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].effect_summary.as_deref(), Some("llm+io"));

    let canceled = queue.cancel(&job.id).unwrap();
    assert_eq!(canceled.status, QueueJobStatus::Canceled);
}

#[test]
fn durable_queue_run_one_persists_success() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let job = queue
        .enqueue_typed(
            "daily_brief",
            serde_json::json!({"user": "u1"}),
            Some("DailyBriefInput".to_string()),
            2,
            0.5,
            Some("llm+db".to_string()),
            Some("replay:job".to_string()),
        )
        .unwrap();

    let ran = queue
        .run_one_with_output(
            Some("DailyBriefOutput".to_string()),
            Some("sha256:daily-output".to_string()),
        )
        .unwrap()
        .expect("pending job");
    assert_eq!(ran.id, job.id);
    assert_eq!(ran.status, QueueJobStatus::Succeeded);
    assert_eq!(ran.attempts, 1);
    assert_eq!(ran.input_schema.as_deref(), Some("DailyBriefInput"));
    assert_eq!(ran.output_kind.as_deref(), Some("DailyBriefOutput"));
    assert_eq!(
        ran.output_fingerprint.as_deref(),
        Some("sha256:daily-output")
    );

    let fetched = queue.get(&job.id).unwrap().unwrap();
    assert_eq!(fetched.status, QueueJobStatus::Succeeded);
    assert_eq!(fetched.attempts, 1);
    assert_eq!(fetched.output_kind.as_deref(), Some("DailyBriefOutput"));
    assert!(queue.run_one().unwrap().is_none());
}

#[test]
fn durable_queue_idempotency_key_collapses_duplicate_jobs() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let first = queue
        .enqueue_typed_idempotent(
            "charge_card",
            serde_json::json!({"invoice": "i1"}),
            Some("ChargeInput".to_string()),
            1,
            10.0,
            Some("payment".to_string()),
            Some("replay:charge:i1".to_string()),
            Some("charge:i1".to_string()),
            None,
        )
        .unwrap();
    let duplicate = queue
        .enqueue_typed_idempotent(
            "charge_card",
            serde_json::json!({"invoice": "i1", "changed": true}),
            Some("ChargeInput".to_string()),
            1,
            10.0,
            Some("payment".to_string()),
            Some("replay:charge:i1:duplicate".to_string()),
            Some("charge:i1".to_string()),
            None,
        )
        .unwrap();

    assert_eq!(first.id, duplicate.id);
    assert_eq!(duplicate.payload["invoice"], "i1");
    assert!(duplicate.payload.get("changed").is_none());
    assert_eq!(duplicate.idempotency_key.as_deref(), Some("charge:i1"));
    assert_eq!(queue.list().unwrap().len(), 1);
}

#[test]
fn durable_queue_records_retry_wait_and_dead_letter() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let job = queue
        .enqueue(
            "send_email",
            serde_json::json!({"draft": "d1"}),
            1,
            1.0,
            Some("llm+email".to_string()),
            Some("replay:email".to_string()),
        )
        .unwrap();

    let retry = queue
        .run_one_failed("provider_timeout", "sha256:failure-1", 1000)
        .unwrap()
        .expect("retry job");
    assert_eq!(retry.id, job.id);
    assert_eq!(retry.status, QueueJobStatus::RetryWait);
    assert_eq!(retry.attempts, 1);
    assert_eq!(retry.failure_kind.as_deref(), Some("provider_timeout"));
    assert_eq!(
        retry.failure_fingerprint.as_deref(),
        Some("sha256:failure-1")
    );
    assert!(retry.next_run_ms.is_some());

    let stored = queue.get(&job.id).unwrap().unwrap();
    assert_eq!(stored.status, QueueJobStatus::RetryWait);
    assert!(
        queue.run_one().unwrap().is_none(),
        "backoff should delay retry"
    );

    let terminal = queue
        .enqueue(
            "send_email",
            serde_json::json!({"draft": "d2"}),
            0,
            1.0,
            Some("llm+email".to_string()),
            Some("replay:email:dead".to_string()),
        )
        .unwrap();
    let dead = queue
        .run_one_failed("provider_timeout", "sha256:failure-2", 0)
        .unwrap()
        .expect("dead-letter job");
    assert_eq!(dead.id, terminal.id);
    assert_eq!(dead.status, QueueJobStatus::DeadLettered);
    assert_eq!(dead.attempts, 1);
    assert_eq!(
        dead.failure_fingerprint.as_deref(),
        Some("sha256:failure-2")
    );
    assert!(dead.next_run_ms.is_none());
    let dlq = queue.dead_lettered().unwrap();
    assert_eq!(dlq.len(), 1);
    assert_eq!(dlq[0].id, terminal.id);
}
