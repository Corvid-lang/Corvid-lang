use super::*;

#[test]
fn durable_queue_enters_approval_wait_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.sqlite");
    let approval_expires_ms = now_ms().saturating_add(600_000);
    let job_id = {
        let queue = DurableQueueRuntime::open(&path).unwrap();
        let job = queue
            .enqueue(
                "send_email",
                serde_json::json!({"draft": "d1"}),
                1,
                0.25,
                Some("email:write approve:send".to_string()),
                Some("replay:email:d1".to_string()),
            )
            .unwrap();
        let leased = queue
            .lease_next_at("worker-a", 60_000, now_ms())
            .unwrap()
            .expect("lease");
        assert_eq!(leased.id, job.id);
        let waiting = queue
            .enter_approval_wait(
                &job.id,
                "worker-a",
                "approval:send:d1",
                approval_expires_ms,
                "send external email draft d1",
            )
            .unwrap();
        assert_eq!(waiting.status, QueueJobStatus::ApprovalWait);
        assert_eq!(waiting.approval_id.as_deref(), Some("approval:send:d1"));
        assert_eq!(waiting.approval_expires_ms, Some(approval_expires_ms));
        assert_eq!(
            waiting.approval_reason.as_deref(),
            Some("send external email draft d1")
        );
        assert!(waiting.lease_owner.is_none());
        assert!(waiting.lease_expires_ms.is_none());
        assert!(
            queue
                .lease_next_at("worker-b", 60_000, now_ms())
                .unwrap()
                .is_none(),
            "approval-wait jobs must not be leased as runnable work"
        );
        job.id
    };

    let queue = DurableQueueRuntime::open(&path).unwrap();
    let stored = queue.get(&job_id).unwrap().expect("stored job");
    assert_eq!(stored.status, QueueJobStatus::ApprovalWait);
    assert_eq!(stored.approval_id.as_deref(), Some("approval:send:d1"));
    assert_eq!(stored.approval_expires_ms, Some(approval_expires_ms));
    assert_eq!(queue.approval_waiting().unwrap().len(), 1);
    assert!(queue.lease_next("worker-c", 60_000).unwrap().is_none());
}

#[test]
fn durable_queue_approval_decisions_resume_or_stop_with_audit() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let approved = queue
        .enqueue(
            "send_email",
            serde_json::json!({"draft": "a"}),
            1,
            0.25,
            None,
            None,
        )
        .unwrap();
    let denied = queue
        .enqueue(
            "send_email",
            serde_json::json!({"draft": "d"}),
            1,
            0.25,
            None,
            None,
        )
        .unwrap();
    let expired = queue
        .enqueue(
            "send_email",
            serde_json::json!({"draft": "e"}),
            1,
            0.25,
            None,
            None,
        )
        .unwrap();

    for (job, approval_id, expires_ms) in [
        (&approved, "approval:a", 20_000),
        (&denied, "approval:d", 20_000),
        (&expired, "approval:e", 10_000),
    ] {
        let leased = queue
            .lease_next_at("worker-a", 60_000, 1_000)
            .unwrap()
            .expect("lease");
        assert_eq!(leased.id, job.id);
        queue
            .enter_approval_wait(
                &job.id,
                "worker-a",
                approval_id,
                expires_ms,
                format!("decide {approval_id}"),
            )
            .unwrap();
    }

    let resumed = queue
        .decide_approval_wait_at(
            &approved.id,
            "approval:a",
            JobApprovalDecision::Approve,
            "reviewer:u1",
            Some("approved by policy".to_string()),
            12_000,
        )
        .unwrap();
    assert_eq!(resumed.status, QueueJobStatus::Pending);
    assert_eq!(resumed.next_run_ms, Some(12_000));
    let runnable = queue
        .lease_next_at("worker-b", 60_000, 12_001)
        .unwrap()
        .expect("approved job resumes");
    assert_eq!(runnable.id, approved.id);

    let stopped = queue
        .decide_approval_wait_at(
            &denied.id,
            "approval:d",
            JobApprovalDecision::Deny,
            "reviewer:u1",
            Some("recipient mismatch".to_string()),
            12_002,
        )
        .unwrap();
    assert_eq!(stopped.status, QueueJobStatus::ApprovalDenied);
    assert!(stopped.next_run_ms.is_none());

    let too_early = queue.decide_approval_wait_at(
        &expired.id,
        "approval:e",
        JobApprovalDecision::Expire,
        "system",
        Some("timer fired early".to_string()),
        9_999,
    );
    assert!(too_early.is_err());
    let expired_job = queue
        .decide_approval_wait_at(
            &expired.id,
            "approval:e",
            JobApprovalDecision::Expire,
            "system",
            Some("approval expired".to_string()),
            10_001,
        )
        .unwrap();
    assert_eq!(expired_job.status, QueueJobStatus::ApprovalExpired);

    let approved_events = queue.job_audit_events(&approved.id).unwrap();
    assert_eq!(approved_events.len(), 1);
    assert_eq!(approved_events[0].event_kind, "approval_approve");
    assert_eq!(approved_events[0].status_before, "approval_wait");
    assert_eq!(approved_events[0].status_after, "pending");
    assert_eq!(
        approved_events[0].approval_id.as_deref(),
        Some("approval:a")
    );
    let denied_events = queue.job_audit_events(&denied.id).unwrap();
    assert_eq!(denied_events[0].event_kind, "approval_deny");
    assert_eq!(denied_events[0].status_after, "approval_denied");
    let expired_events = queue.job_audit_events(&expired.id).unwrap();
    assert_eq!(expired_events[0].event_kind, "approval_expire");
    assert_eq!(expired_events[0].status_after, "approval_expired");
}
