use super::*;

impl DurableQueueRuntime {
    pub fn get(&self, id: &str) -> Result<Option<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms
                 from queue_jobs where id = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare durable queue read: {err}")))?;
        let mut rows = stmt.query(params![id]).map_err(|err| {
            RuntimeError::Other(format!("failed to query durable queue job: {err}"))
        })?;
        if let Some(row) = rows.next().map_err(|err| {
            RuntimeError::Other(format!("failed to read durable queue row: {err}"))
        })? {
            Ok(Some(read_job_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode durable queue job: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms
                 from queue_jobs where idempotency_key = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare idempotency read: {err}")))?;
        let mut rows = stmt.query(params![idempotency_key]).map_err(|err| {
            RuntimeError::Other(format!("failed to query idempotency key: {err}"))
        })?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read idempotency row: {err}")))?
        {
            Ok(Some(read_job_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode idempotent job: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn list(&self) -> Result<Vec<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms
                 from queue_jobs order by created_ms, id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare durable queue list: {err}")))?;
        let rows = stmt.query_map([], read_job_row).map_err(|err| {
            RuntimeError::Other(format!("failed to list durable queue jobs: {err}"))
        })?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode durable queue job: {err}"))
            })?);
        }
        Ok(jobs)
    }

    pub fn dead_lettered(&self) -> Result<Vec<QueueJob>, RuntimeError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|job| job.status == QueueJobStatus::DeadLettered)
            .collect())
    }

    pub fn set_paused(&self, paused: bool, reason: Option<&str>) -> Result<(), RuntimeError> {
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_controls (name, value, reason, updated_ms)
                 values ('paused', ?1, ?2, ?3)
                 on conflict(name) do update set
                    value = excluded.value,
                    reason = excluded.reason,
                    updated_ms = excluded.updated_ms",
                params![if paused { "true" } else { "false" }, reason, now as i64],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to set queue pause state: {err}"))
            })?;
        Ok(())
    }

    pub fn is_paused(&self) -> Result<bool, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let paused = conn
            .query_row(
                "select value from queue_controls where name = 'paused'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| {
                RuntimeError::Other(format!("failed to read queue pause state: {err}"))
            })?;
        Ok(paused.as_deref() == Some("true"))
    }

    pub fn drain_active_leases(&self, reason: Option<&str>) -> Result<usize, RuntimeError> {
        self.set_paused(true, reason)?;
        let now = now_ms();
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = 'pending',
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?1
                 where status = 'leased'",
                params![now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to drain active leases: {err}")))?;
        Ok(updated)
    }

    pub fn cancel(&self, id: &str) -> Result<QueueJob, RuntimeError> {
        let now = now_ms();
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs set status = 'canceled', updated_ms = ?2 where id = ?1",
                params![id, now as i64],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to cancel durable queue job: {err}"))
            })?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` not found"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }
}
