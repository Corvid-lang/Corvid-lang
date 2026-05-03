use super::*;

impl DurableQueueRuntime {
    pub fn run_one(&self) -> Result<Option<QueueJob>, RuntimeError> {
        self.run_one_with_output(None, None)
    }

    pub fn enter_approval_wait(
        &self,
        id: &str,
        worker_id: &str,
        approval_id: impl Into<String>,
        approval_expires_ms: u64,
        approval_reason: impl Into<String>,
    ) -> Result<QueueJob, RuntimeError> {
        let approval_id = approval_id.into();
        let approval_reason = approval_reason.into();
        if approval_id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval id must not be empty".to_string(),
            ));
        }
        if approval_reason.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval reason must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = 'approval_wait',
                     approval_id = ?3,
                     approval_expires_ms = ?4,
                     approval_reason = ?5,
                     next_run_ms = null,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?6
                 where id = ?1 and status = 'leased' and lease_owner = ?2",
                params![
                    id,
                    worker_id,
                    approval_id,
                    approval_expires_ms as i64,
                    approval_reason,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to enter approval wait: {err}")))?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn decide_approval_wait(
        &self,
        id: &str,
        approval_id: &str,
        decision: JobApprovalDecision,
        actor: impl Into<String>,
        reason: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        self.decide_approval_wait_at(id, approval_id, decision, actor, reason, now_ms())
    }

    pub fn decide_approval_wait_at(
        &self,
        id: &str,
        approval_id: &str,
        decision: JobApprovalDecision,
        actor: impl Into<String>,
        reason: Option<String>,
        now: u64,
    ) -> Result<QueueJob, RuntimeError> {
        let actor = actor.into();
        if actor.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval actor must not be empty".to_string(),
            ));
        }
        if approval_id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval id must not be empty".to_string(),
            ));
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!(
                "failed to start approval decision transaction: {err}"
            ))
        })?;
        let (stored_status, stored_approval_id, stored_expires_ms) = tx
            .query_row(
                "select status, approval_id, approval_expires_ms from queue_jobs where id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .map_err(|err| {
                RuntimeError::Other(format!(
                    "std.queue job `{id}` not found or unreadable: {err}"
                ))
            })?;
        if parse_status(&stored_status) != QueueJobStatus::ApprovalWait {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not waiting on approval"
            )));
        }
        if stored_approval_id.as_deref() != Some(approval_id) {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is waiting on a different approval id"
            )));
        }
        if decision == JobApprovalDecision::Expire {
            let Some(expires_ms) = stored_expires_ms else {
                return Err(RuntimeError::Other(format!(
                    "std.queue job `{id}` approval has no expiry"
                )));
            };
            if (expires_ms as u64) > now {
                return Err(RuntimeError::Other(format!(
                    "std.queue job `{id}` approval `{approval_id}` has not expired"
                )));
            }
        }

        let (status_after, next_run_ms) = match decision {
            JobApprovalDecision::Approve => (QueueJobStatus::Pending, Some(now)),
            JobApprovalDecision::Deny => (QueueJobStatus::ApprovalDenied, None),
            JobApprovalDecision::Expire => (QueueJobStatus::ApprovalExpired, None),
        };
        let updated = tx
            .execute(
                "update queue_jobs
                 set status = ?3,
                     next_run_ms = ?4,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?5
                 where id = ?1 and status = 'approval_wait' and approval_id = ?2",
                params![
                    id,
                    approval_id,
                    status_after.as_str(),
                    next_run_ms.map(|value| value as i64),
                    now as i64,
                ],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to apply approval decision: {err}"))
            })?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` approval decision raced with another transition"
            )));
        }
        insert_job_audit_event(
            &tx,
            id,
            &format!("approval_{}", decision.as_str()),
            &actor,
            Some(approval_id),
            &stored_status,
            status_after.as_str(),
            reason.as_deref(),
            now,
        )?;
        tx.commit().map_err(|err| {
            RuntimeError::Other(format!("failed to commit approval decision: {err}"))
        })?;
        drop(conn);
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn run_one_with_output(
        &self,
        output_kind: Option<String>,
        output_fingerprint: Option<String>,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let Some(job) = self.lease_next("corvid-run-one", 300_000)? else {
            return Ok(None);
        };
        self.complete_leased(&job.id, "corvid-run-one", output_kind, output_fingerprint)
            .map(Some)
    }

    pub fn run_one_failed(
        &self,
        failure_kind: impl Into<String>,
        failure_fingerprint: impl Into<String>,
        base_delay_ms: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let Some(job) = self.lease_next("corvid-run-one", 300_000)? else {
            return Ok(None);
        };
        self.fail_leased(
            &job.id,
            "corvid-run-one",
            failure_kind,
            failure_fingerprint,
            base_delay_ms,
        )
        .map(Some)
    }
}
