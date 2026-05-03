use super::*;

#[test]
fn durable_queue_leases_prevent_duplicate_workers() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let job = queue
        .enqueue(
            "dangerous_send",
            serde_json::json!({"draft": "d1"}),
            1,
            2.0,
            Some("email_send".to_string()),
            Some("replay:send:d1".to_string()),
        )
        .unwrap();

    let leased = queue
        .lease_next_at("worker-a", 60_000, 1_000_000)
        .unwrap()
        .expect("worker-a lease");
    assert_eq!(leased.id, job.id);
    assert_eq!(leased.status, QueueJobStatus::Leased);
    assert_eq!(leased.lease_owner.as_deref(), Some("worker-a"));
    assert_eq!(leased.lease_expires_ms, Some(1_060_000));

    assert!(
        queue
            .lease_next_at("worker-b", 60_000, 1_000_001)
            .unwrap()
            .is_none(),
        "second worker must not lease an active lease"
    );
    let wrong_owner = queue.complete_leased(&job.id, "worker-b", None, None);
    assert!(wrong_owner.is_err(), "non-owner completion must fail");

    let succeeded = queue
        .complete_leased(
            &job.id,
            "worker-a",
            Some("SendOutput".to_string()),
            Some("sha256:send".to_string()),
        )
        .unwrap();
    assert_eq!(succeeded.status, QueueJobStatus::Succeeded);
    assert!(succeeded.lease_owner.is_none());
    assert!(succeeded.lease_expires_ms.is_none());
    assert_eq!(succeeded.attempts, 1);
}

#[test]
fn durable_queue_expired_lease_can_be_reclaimed() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let job = queue
        .enqueue("brief", serde_json::json!({}), 1, 0.1, None, None)
        .unwrap();

    let first = queue
        .lease_next_at("worker-a", 10, 2_000)
        .unwrap()
        .expect("first lease");
    assert_eq!(first.id, job.id);

    let reclaimed = queue
        .lease_next_at("worker-b", 10, 2_011)
        .unwrap()
        .expect("reclaimed lease");
    assert_eq!(reclaimed.id, job.id);
    assert_eq!(reclaimed.lease_owner.as_deref(), Some("worker-b"));
    assert_eq!(reclaimed.lease_expires_ms, Some(2_021));
}

#[test]
fn durable_queue_enforces_global_and_task_concurrency_limits() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    queue.set_global_concurrency_limit(1).unwrap();
    queue.set_task_concurrency_limit("email", 1).unwrap();
    queue
        .enqueue("email", serde_json::json!({"n": 1}), 1, 0.1, None, None)
        .unwrap();
    queue
        .enqueue("email", serde_json::json!({"n": 2}), 1, 0.1, None, None)
        .unwrap();
    queue
        .enqueue("brief", serde_json::json!({"n": 3}), 1, 0.1, None, None)
        .unwrap();

    let first = queue
        .lease_next_at("worker-a", 60_000, 5_000)
        .unwrap()
        .expect("first lease");
    assert_eq!(first.task, "email");
    assert!(
        queue
            .lease_next_at("worker-b", 60_000, 5_001)
            .unwrap()
            .is_none(),
        "global limit should block every other task while one lease is active"
    );

    queue
        .complete_leased(&first.id, "worker-a", None, None)
        .unwrap();
    let second = queue
        .lease_next_at("worker-b", 60_000, 5_002)
        .unwrap()
        .expect("second email lease");
    assert_eq!(second.task, "email");
    assert!(
        queue
            .lease_next_at("worker-c", 60_000, 5_003)
            .unwrap()
            .is_none(),
        "task limit should block another email lease"
    );

    let limits = queue.list_concurrency_limits().unwrap();
    assert_eq!(limits.len(), 2);
    assert!(limits
        .iter()
        .any(|limit| limit.scope == "global" && limit.limit == 1));
    assert!(limits
        .iter()
        .any(|limit| limit.scope == "task:email" && limit.limit == 1));
}
