use super::*;

#[test]
fn durable_queue_enforces_loop_budget_limits_with_audit() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let job = queue
        .enqueue(
            "daily_brief_agent",
            serde_json::json!({"user": "u1"}),
            1,
            0.20,
            Some("llm+tools".to_string()),
            Some("replay:brief:u1".to_string()),
        )
        .unwrap();
    queue
        .set_loop_limits(&job.id, Some(3), Some(1_000), Some(0.20), Some(2))
        .unwrap();

    let first = queue
        .record_loop_usage_at(&job.id, 1, 250, 0.05, 1, "worker-a", 10_000)
        .unwrap();
    assert!(first.violated_bounds.is_empty());
    assert_eq!(first.usage.steps, 1);
    assert_eq!(
        queue.get(&job.id).unwrap().unwrap().status,
        QueueJobStatus::Pending
    );

    let exceeded = queue
        .record_loop_usage_at(&job.id, 3, 900, 0.16, 2, "worker-a", 10_100)
        .unwrap();
    assert_eq!(exceeded.usage.steps, 4);
    assert_eq!(exceeded.usage.wall_ms, 1_150);
    assert_eq!(exceeded.usage.tool_calls, 3);
    assert!(exceeded
        .violated_bounds
        .iter()
        .any(|bound| bound.starts_with("max_steps:4>3")));
    assert!(exceeded
        .violated_bounds
        .iter()
        .any(|bound| bound.starts_with("max_wall_ms:1150>1000")));
    assert!(exceeded
        .violated_bounds
        .iter()
        .any(|bound| bound.starts_with("max_spend_usd:0.210000>0.200000")));
    assert!(exceeded
        .violated_bounds
        .iter()
        .any(|bound| bound.starts_with("max_tool_calls:3>2")));

    let stopped = queue.get(&job.id).unwrap().unwrap();
    assert_eq!(stopped.status, QueueJobStatus::LoopBudgetExceeded);
    assert_eq!(stopped.failure_kind.as_deref(), Some("loop_bound_exceeded"));
    assert!(stopped
        .failure_fingerprint
        .as_deref()
        .unwrap_or_default()
        .contains("max_steps:4>3"));
    let audit = queue.job_audit_events(&job.id).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].event_kind, "loop_bound_exceeded");
    assert_eq!(audit[0].actor, "worker-a");
    assert_eq!(audit[0].status_after, "loop_budget_exceeded");
    assert!(queue
        .record_loop_usage_at(&job.id, 1, 1, 0.01, 0, "worker-a", 10_200)
        .is_err());
}

#[test]
fn durable_queue_escalates_or_terminates_stalled_loops_with_audit() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let escalated = queue
        .enqueue(
            "agent_loop",
            serde_json::json!({"n": 1}),
            1,
            0.1,
            None,
            None,
        )
        .unwrap();
    let terminated = queue
        .enqueue(
            "agent_loop",
            serde_json::json!({"n": 2}),
            1,
            0.1,
            None,
            None,
        )
        .unwrap();

    queue
        .set_stall_policy(&escalated.id, 1_000, JobStallAction::Escalate)
        .unwrap();
    queue
        .set_stall_policy(&terminated.id, 1_000, JobStallAction::Terminate)
        .unwrap();
    queue
        .record_loop_heartbeat_at(
            &escalated.id,
            "worker-a",
            Some("step 1".to_string()),
            10_000,
        )
        .unwrap();
    queue
        .record_loop_heartbeat_at(
            &terminated.id,
            "worker-b",
            Some("step 1".to_string()),
            20_000,
        )
        .unwrap();

    let healthy = queue
        .check_stall_at(&escalated.id, "watchdog", 10_500)
        .unwrap();
    assert!(!healthy.stalled);
    assert_eq!(healthy.elapsed_ms, 500);

    let stalled = queue
        .check_stall_at(&escalated.id, "watchdog", 11_001)
        .unwrap();
    assert!(stalled.stalled);
    assert_eq!(stalled.action_taken.as_deref(), Some("escalate"));
    let job = queue.get(&escalated.id).unwrap().unwrap();
    assert_eq!(job.status, QueueJobStatus::LoopStallEscalated);
    assert_eq!(job.failure_kind.as_deref(), Some("loop_stalled"));
    let audit = queue.job_audit_events(&escalated.id).unwrap();
    assert_eq!(audit[0].event_kind, "loop_stalled_escalate");
    assert_eq!(audit[0].status_after, "loop_stall_escalated");
    assert!(audit[0]
        .reason
        .as_deref()
        .unwrap_or_default()
        .contains("elapsed_ms=1001"));

    let terminated_check = queue
        .check_stall_at(&terminated.id, "watchdog", 21_001)
        .unwrap();
    assert!(terminated_check.stalled);
    assert_eq!(terminated_check.action_taken.as_deref(), Some("terminate"));
    let job = queue.get(&terminated.id).unwrap().unwrap();
    assert_eq!(job.status, QueueJobStatus::LoopStallTerminated);
    let audit = queue.job_audit_events(&terminated.id).unwrap();
    assert_eq!(audit[0].event_kind, "loop_stalled_terminate");
    assert_eq!(audit[0].status_after, "loop_stall_terminated");
}
