use super::*;

#[test]
fn durable_queue_records_ordered_agent_checkpoints() {
    let queue = DurableQueueRuntime::open_in_memory().unwrap();
    let job = queue
        .enqueue(
            "agent_run",
            serde_json::json!({"goal": "brief"}),
            1,
            0.5,
            None,
            None,
        )
        .unwrap();

    let step = queue
        .record_checkpoint(
            &job.id,
            JobCheckpointKind::AgentStep,
            "plan",
            serde_json::json!({"step": 1}),
            Some("sha256:plan".to_string()),
        )
        .unwrap();
    let tool = queue
        .record_checkpoint(
            &job.id,
            JobCheckpointKind::ToolResult,
            "gmail.search",
            serde_json::json!({"result_count": 3}),
            Some("sha256:gmail".to_string()),
        )
        .unwrap();
    let partial = queue
        .record_checkpoint(
            &job.id,
            JobCheckpointKind::PartialOutput,
            "draft",
            serde_json::json!({"chars": 120}),
            None,
        )
        .unwrap();

    assert_eq!(step.sequence, 1);
    assert_eq!(tool.sequence, 2);
    assert_eq!(partial.sequence, 3);
    let checkpoints = queue.list_checkpoints(&job.id).unwrap();
    assert_eq!(checkpoints.len(), 3);
    assert_eq!(checkpoints[0].kind, JobCheckpointKind::AgentStep);
    assert_eq!(checkpoints[1].label, "gmail.search");
    assert_eq!(checkpoints[2].payload["chars"], 120);
}

#[test]
fn durable_queue_resume_state_survives_restart_and_expired_lease() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.sqlite");
    let job_id = {
        let queue = DurableQueueRuntime::open(&path).unwrap();
        let job = queue
            .enqueue(
                "agent_run",
                serde_json::json!({"goal": "brief"}),
                1,
                0.5,
                None,
                None,
            )
            .unwrap();
        let leased = queue
            .lease_next_at("worker-a", 10, 10_000)
            .unwrap()
            .expect("lease");
        assert_eq!(leased.id, job.id);
        queue
            .record_checkpoint(
                &job.id,
                JobCheckpointKind::AgentStep,
                "plan",
                serde_json::json!({"step": 1}),
                Some("sha256:plan".to_string()),
            )
            .unwrap();
        queue
            .record_checkpoint(
                &job.id,
                JobCheckpointKind::ToolResult,
                "gmail.search",
                serde_json::json!({"result_count": 3}),
                Some("sha256:gmail".to_string()),
            )
            .unwrap();
        job.id
    };

    let queue = DurableQueueRuntime::open(&path).unwrap();
    let resume = queue.resume_state(&job_id).unwrap();
    assert_eq!(resume.job.status, QueueJobStatus::Leased);
    assert_eq!(resume.checkpoints.len(), 2);
    assert_eq!(resume.next_sequence, 3);
    assert_eq!(
        resume
            .last_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.label.as_str()),
        Some("gmail.search")
    );

    let reclaimed = queue
        .lease_next_at("worker-b", 10, 10_011)
        .unwrap()
        .expect("reclaimed after restart");
    assert_eq!(reclaimed.id, job_id);
    assert_eq!(reclaimed.lease_owner.as_deref(), Some("worker-b"));
}
