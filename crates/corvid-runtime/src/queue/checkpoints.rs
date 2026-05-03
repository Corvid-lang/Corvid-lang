use super::*;

impl DurableQueueRuntime {
    pub fn retry_job(&self, id: &str) -> Result<QueueJob, RuntimeError> {
        let job = self
            .get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))?;
        if matches!(job.status, QueueJobStatus::Pending | QueueJobStatus::Leased) {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is already runnable or leased"
            )));
        }
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = 'pending',
                     next_run_ms = ?2,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?2
                 where id = ?1",
                params![id, now as i64],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to retry durable queue job: {err}"))
            })?;
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn approval_waiting(&self) -> Result<Vec<QueueJob>, RuntimeError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|job| job.status == QueueJobStatus::ApprovalWait)
            .collect())
    }

    pub fn job_audit_events(&self, job_id: &str) -> Result<Vec<JobAuditEvent>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, job_id, event_kind, actor, approval_id, status_before, status_after, reason, created_ms
                 from queue_job_audit_events
                 where job_id = ?1
                 order by created_ms, id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare job audit read: {err}")))?;
        let rows = stmt
            .query_map(params![job_id], read_job_audit_row)
            .map_err(|err| {
                RuntimeError::Other(format!("failed to list job audit events: {err}"))
            })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode job audit event: {err}"))
            })?);
        }
        Ok(events)
    }

    pub fn record_checkpoint(
        &self,
        job_id: &str,
        kind: JobCheckpointKind,
        label: impl Into<String>,
        payload: Value,
        payload_fingerprint: Option<String>,
    ) -> Result<JobCheckpoint, RuntimeError> {
        if self.get(job_id)?.is_none() {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{job_id}` not found"
            )));
        }
        let label = label.into();
        if label.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue checkpoint label must not be empty".to_string(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!("failed to start checkpoint transaction: {err}"))
        })?;
        let sequence = tx
            .query_row(
                "select coalesce(max(sequence), 0) + 1 from queue_job_checkpoints where job_id = ?1",
                params![job_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| RuntimeError::Other(format!("failed to allocate checkpoint sequence: {err}")))?
            as u64;
        let id = format!("{job_id}:checkpoint:{sequence}");
        let payload_json = serde_json::to_string(&payload).map_err(|err| {
            RuntimeError::Other(format!("failed to serialize checkpoint payload: {err}"))
        })?;
        let now = now_ms();
        tx.execute(
            "insert into queue_job_checkpoints
             (id, job_id, sequence, kind, label, payload_json, payload_fingerprint, created_ms)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                job_id,
                sequence as i64,
                kind.as_str(),
                label,
                payload_json,
                payload_fingerprint,
                now as i64,
            ],
        )
        .map_err(|err| RuntimeError::Other(format!("failed to insert checkpoint: {err}")))?;
        tx.commit()
            .map_err(|err| RuntimeError::Other(format!("failed to commit checkpoint: {err}")))?;
        Ok(JobCheckpoint {
            id,
            job_id: job_id.to_string(),
            sequence,
            kind,
            label,
            payload,
            payload_fingerprint,
            created_ms: now,
        })
    }

    pub fn list_checkpoints(&self, job_id: &str) -> Result<Vec<JobCheckpoint>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, job_id, sequence, kind, label, payload_json, payload_fingerprint, created_ms
                 from queue_job_checkpoints where job_id = ?1 order by sequence",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare checkpoint list: {err}")))?;
        let rows = stmt
            .query_map(params![job_id], read_checkpoint_row)
            .map_err(|err| RuntimeError::Other(format!("failed to list checkpoints: {err}")))?;
        let mut checkpoints = Vec::new();
        for row in rows {
            checkpoints.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode checkpoint: {err}"))
            })?);
        }
        Ok(checkpoints)
    }

    pub fn resume_state(&self, job_id: &str) -> Result<JobResumeState, RuntimeError> {
        let job = self
            .get(job_id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{job_id}` not found")))?;
        let checkpoints = self.list_checkpoints(job_id)?;
        let last_checkpoint = checkpoints.last().cloned();
        let next_sequence = last_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.sequence.saturating_add(1))
            .unwrap_or(1);
        Ok(JobResumeState {
            job,
            checkpoints,
            last_checkpoint,
            next_sequence,
        })
    }
}
