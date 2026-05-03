use super::*;

pub(super) fn insert_job_audit_event(
    tx: &rusqlite::Transaction<'_>,
    job_id: &str,
    event_kind: &str,
    actor: &str,
    approval_id: Option<&str>,
    status_before: &str,
    status_after: &str,
    reason: Option<&str>,
    created_ms: u64,
) -> Result<(), RuntimeError> {
    let next = tx
        .query_row(
            "select coalesce(max(cast(substr(id, 7) as integer)), 0) + 1 from queue_job_audit_events where id like 'audit_%'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|err| RuntimeError::Other(format!("failed to allocate job audit id: {err}")))?;
    let id = format!("audit_{}", next.max(1));
    tx.execute(
        "insert into queue_job_audit_events
         (id, job_id, event_kind, actor, approval_id, status_before, status_after, reason, created_ms)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            job_id,
            event_kind,
            actor,
            approval_id,
            status_before,
            status_after,
            reason,
            created_ms as i64,
        ],
    )
    .map_err(|err| RuntimeError::Other(format!("failed to insert job audit event: {err}")))?;
    Ok(())
}

fn eligible_to_run(job: &QueueJob) -> bool {
    match job.status {
        QueueJobStatus::Pending => job.next_run_ms.map(|next| next <= now_ms()).unwrap_or(true),
        QueueJobStatus::RetryWait => job.next_run_ms.map(|next| next <= now_ms()).unwrap_or(true),
        _ => false,
    }
}
