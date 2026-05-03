use super::*;

impl DurableQueueRuntime {
    pub fn set_global_concurrency_limit(
        &self,
        limit: u64,
    ) -> Result<QueueConcurrencyLimit, RuntimeError> {
        self.set_concurrency_limit("global", limit)
    }

    pub fn set_task_concurrency_limit(
        &self,
        task: &str,
        limit: u64,
    ) -> Result<QueueConcurrencyLimit, RuntimeError> {
        if task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue task concurrency limit name must not be empty".to_string(),
            ));
        }
        self.set_concurrency_limit(&format!("task:{task}"), limit)
    }

    pub fn list_concurrency_limits(&self) -> Result<Vec<QueueConcurrencyLimit>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select scope, max_leased, updated_ms from queue_concurrency_limits order by scope",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to prepare concurrency limit list: {err}"))
            })?;
        let rows = stmt
            .query_map([], |row| {
                Ok(QueueConcurrencyLimit {
                    scope: row.get(0)?,
                    limit: row.get::<_, i64>(1)? as u64,
                    updated_ms: row.get::<_, i64>(2)? as u64,
                })
            })
            .map_err(|err| {
                RuntimeError::Other(format!("failed to list concurrency limits: {err}"))
            })?;
        let mut limits = Vec::new();
        for row in rows {
            limits.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode concurrency limit: {err}"))
            })?);
        }
        Ok(limits)
    }

    fn set_concurrency_limit(
        &self,
        scope: &str,
        limit: u64,
    ) -> Result<QueueConcurrencyLimit, RuntimeError> {
        if limit == 0 {
            return Err(RuntimeError::Other(
                "std.queue concurrency limit must be at least 1".to_string(),
            ));
        }
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_concurrency_limits (scope, max_leased, updated_ms)
                 values (?1, ?2, ?3)
                 on conflict(scope) do update set max_leased = excluded.max_leased, updated_ms = excluded.updated_ms",
                params![scope, limit as i64, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to set concurrency limit: {err}")))?;
        Ok(QueueConcurrencyLimit {
            scope: scope.to_string(),
            limit,
            updated_ms: now,
        })
    }

    pub fn lease_next(
        &self,
        worker_id: impl Into<String>,
        ttl_ms: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        self.lease_next_at(worker_id, ttl_ms, now_ms())
    }

    pub fn lease_next_at(
        &self,
        worker_id: impl Into<String>,
        ttl_ms: u64,
        now: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        if self.is_paused()? {
            return Ok(None);
        }
        let worker_id = worker_id.into();
        if worker_id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue lease worker id must not be empty".to_string(),
            ));
        }
        let lease_expires = now.saturating_add(ttl_ms.max(1));
        for candidate in self
            .list()?
            .into_iter()
            .filter(|job| eligible_to_lease(job, now))
        {
            if !self.concurrency_allows(&candidate.task, now)? {
                continue;
            }
            let updated = self
                .conn
                .lock()
                .unwrap()
                .execute(
                    "update queue_jobs
                     set status = 'leased', lease_owner = ?2, lease_expires_ms = ?3, updated_ms = ?4
                     where id = ?1
                       and (
                         status in ('pending', 'retry_wait')
                         or (status = 'leased' and coalesce(lease_expires_ms, 0) <= ?4)
                       )",
                    params![candidate.id, worker_id, lease_expires as i64, now as i64],
                )
                .map_err(|err| {
                    RuntimeError::Other(format!("failed to lease durable queue job: {err}"))
                })?;
            if updated == 1 {
                return self.get(&candidate.id);
            }
        }
        Ok(None)
    }

    fn concurrency_allows(&self, task: &str, now: u64) -> Result<bool, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let global_limit = read_concurrency_limit(&conn, "global")?;
        if let Some(limit) = global_limit {
            let active = count_active_leases(&conn, None, now)?;
            if active >= limit {
                return Ok(false);
            }
        }
        let scope = format!("task:{task}");
        if let Some(limit) = read_concurrency_limit(&conn, &scope)? {
            let active = count_active_leases(&conn, Some(task), now)?;
            if active >= limit {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn complete_leased(
        &self,
        id: &str,
        worker_id: &str,
        output_kind: Option<String>,
        output_fingerprint: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        let now = now_ms();
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = 'succeeded', attempts = attempts + 1, output_kind = ?3, output_fingerprint = ?4,
                     next_run_ms = null, lease_owner = null, lease_expires_ms = null, updated_ms = ?5
                 where id = ?1 and status = 'leased' and lease_owner = ?2",
                params![id, worker_id, output_kind, output_fingerprint, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to complete leased queue job: {err}")))?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn fail_leased(
        &self,
        id: &str,
        worker_id: &str,
        failure_kind: impl Into<String>,
        failure_fingerprint: impl Into<String>,
        base_delay_ms: u64,
    ) -> Result<QueueJob, RuntimeError> {
        let mut job = self
            .get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))?;
        if job.status != QueueJobStatus::Leased || job.lease_owner.as_deref() != Some(worker_id) {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        let now = now_ms();
        job.attempts = job.attempts.saturating_add(1);
        job.failure_kind = Some(failure_kind.into());
        job.failure_fingerprint = Some(failure_fingerprint.into());
        if job.attempts <= job.max_retries {
            job.status = QueueJobStatus::RetryWait;
            job.next_run_ms = Some(now.saturating_add(base_delay_ms.saturating_mul(job.attempts)));
        } else {
            job.status = QueueJobStatus::DeadLettered;
            job.next_run_ms = None;
        }
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = ?3, attempts = ?4, failure_kind = ?5, failure_fingerprint = ?6,
                     next_run_ms = ?7, lease_owner = null, lease_expires_ms = null, updated_ms = ?8
                 where id = ?1 and status = 'leased' and lease_owner = ?2",
                params![
                    id,
                    worker_id,
                    job.status.as_str(),
                    job.attempts as i64,
                    job.failure_kind,
                    job.failure_fingerprint,
                    job.next_run_ms.map(|value| value as i64),
                    now as i64,
                ],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to fail leased queue job: {err}"))
            })?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }
}

fn read_concurrency_limit(conn: &Connection, scope: &str) -> Result<Option<u64>, RuntimeError> {
    let mut stmt = conn
        .prepare("select max_leased from queue_concurrency_limits where scope = ?1")
        .map_err(|err| {
            RuntimeError::Other(format!("failed to prepare concurrency limit read: {err}"))
        })?;
    let mut rows = stmt
        .query(params![scope])
        .map_err(|err| RuntimeError::Other(format!("failed to query concurrency limit: {err}")))?;
    if let Some(row) = rows
        .next()
        .map_err(|err| RuntimeError::Other(format!("failed to read concurrency limit: {err}")))?
    {
        Ok(Some(row.get::<_, i64>(0).map_err(|err| {
            RuntimeError::Other(format!("failed to decode concurrency limit: {err}"))
        })? as u64))
    } else {
        Ok(None)
    }
}

fn count_active_leases(
    conn: &Connection,
    task: Option<&str>,
    now: u64,
) -> Result<u64, RuntimeError> {
    let sql = if task.is_some() {
        "select count(*) from queue_jobs where status = 'leased' and coalesce(lease_expires_ms, 0) > ?1 and task = ?2"
    } else {
        "select count(*) from queue_jobs where status = 'leased' and coalesce(lease_expires_ms, 0) > ?1"
    };
    let count = if let Some(task) = task {
        conn.query_row(sql, params![now as i64, task], |row| row.get::<_, i64>(0))
    } else {
        conn.query_row(sql, params![now as i64], |row| row.get::<_, i64>(0))
    }
    .map_err(|err| RuntimeError::Other(format!("failed to count active leases: {err}")))?;
    Ok(count.max(0) as u64)
}

fn eligible_to_lease(job: &QueueJob, now: u64) -> bool {
    match job.status {
        QueueJobStatus::Pending | QueueJobStatus::RetryWait => {
            job.next_run_ms.map(|next| next <= now).unwrap_or(true)
        }
        QueueJobStatus::Leased => job
            .lease_expires_ms
            .map(|expires| expires <= now)
            .unwrap_or(true),
        _ => false,
    }
}
