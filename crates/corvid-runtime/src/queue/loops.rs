use super::*;

impl DurableQueueRuntime {
    pub fn set_loop_limits(
        &self,
        job_id: &str,
        max_steps: Option<u64>,
        max_wall_ms: Option<u64>,
        max_spend_usd: Option<f64>,
        max_tool_calls: Option<u64>,
    ) -> Result<JobLoopLimits, RuntimeError> {
        if self.get(job_id)?.is_none() {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{job_id}` not found"
            )));
        }
        let max_spend_usd = max_spend_usd.filter(|value| value.is_finite() && *value >= 0.0);
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_job_loop_limits
                 (job_id, max_steps, max_wall_ms, max_spend_usd, max_tool_calls, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6)
                 on conflict(job_id) do update set
                    max_steps = excluded.max_steps,
                    max_wall_ms = excluded.max_wall_ms,
                    max_spend_usd = excluded.max_spend_usd,
                    max_tool_calls = excluded.max_tool_calls,
                    updated_ms = excluded.updated_ms",
                params![
                    job_id,
                    max_steps.map(|value| value as i64),
                    max_wall_ms.map(|value| value as i64),
                    max_spend_usd,
                    max_tool_calls.map(|value| value as i64),
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to set loop limits: {err}")))?;
        self.loop_limits(job_id)?.ok_or_else(|| {
            RuntimeError::Other(format!("std.queue loop limits for `{job_id}` not found"))
        })
    }

    pub fn loop_limits(&self, job_id: &str) -> Result<Option<JobLoopLimits>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select job_id, max_steps, max_wall_ms, max_spend_usd, max_tool_calls, updated_ms
                 from queue_job_loop_limits where job_id = ?1",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to prepare loop limit read: {err}"))
            })?;
        let mut rows = stmt
            .query(params![job_id])
            .map_err(|err| RuntimeError::Other(format!("failed to query loop limits: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read loop limit row: {err}")))?
        {
            Ok(Some(read_loop_limits_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode loop limits: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn loop_usage(&self, job_id: &str) -> Result<Option<JobLoopUsage>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select job_id, steps, wall_ms, spend_usd, tool_calls, updated_ms
                 from queue_job_loop_usage where job_id = ?1",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to prepare loop usage read: {err}"))
            })?;
        let mut rows = stmt
            .query(params![job_id])
            .map_err(|err| RuntimeError::Other(format!("failed to query loop usage: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read loop usage row: {err}")))?
        {
            Ok(Some(read_loop_usage_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode loop usage: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn record_loop_usage(
        &self,
        job_id: &str,
        step_delta: u64,
        wall_ms_delta: u64,
        spend_usd_delta: f64,
        tool_call_delta: u64,
        actor: impl Into<String>,
    ) -> Result<JobLoopUsageReport, RuntimeError> {
        self.record_loop_usage_at(
            job_id,
            step_delta,
            wall_ms_delta,
            spend_usd_delta,
            tool_call_delta,
            actor,
            now_ms(),
        )
    }

    pub fn record_loop_usage_at(
        &self,
        job_id: &str,
        step_delta: u64,
        wall_ms_delta: u64,
        spend_usd_delta: f64,
        tool_call_delta: u64,
        actor: impl Into<String>,
        now: u64,
    ) -> Result<JobLoopUsageReport, RuntimeError> {
        let actor = actor.into();
        if actor.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue loop usage actor must not be empty".to_string(),
            ));
        }
        let spend_usd_delta = if spend_usd_delta.is_finite() && spend_usd_delta > 0.0 {
            spend_usd_delta
        } else {
            0.0
        };
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!("failed to start loop usage transaction: {err}"))
        })?;
        let status_before = tx
            .query_row(
                "select status from queue_jobs where id = ?1",
                params![job_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| {
                RuntimeError::Other(format!(
                    "std.queue job `{job_id}` not found or unreadable: {err}"
                ))
            })?;
        if matches!(
            parse_status(&status_before),
            QueueJobStatus::Succeeded
                | QueueJobStatus::DeadLettered
                | QueueJobStatus::Canceled
                | QueueJobStatus::ApprovalDenied
                | QueueJobStatus::ApprovalExpired
                | QueueJobStatus::LoopBudgetExceeded
        ) {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{job_id}` is terminal and cannot record loop usage"
            )));
        }
        tx.execute(
            "insert into queue_job_loop_usage
             (job_id, steps, wall_ms, spend_usd, tool_calls, updated_ms)
             values (?1, ?2, ?3, ?4, ?5, ?6)
             on conflict(job_id) do update set
                steps = steps + excluded.steps,
                wall_ms = wall_ms + excluded.wall_ms,
                spend_usd = spend_usd + excluded.spend_usd,
                tool_calls = tool_calls + excluded.tool_calls,
                updated_ms = excluded.updated_ms",
            params![
                job_id,
                step_delta as i64,
                wall_ms_delta as i64,
                spend_usd_delta,
                tool_call_delta as i64,
                now as i64,
            ],
        )
        .map_err(|err| RuntimeError::Other(format!("failed to record loop usage: {err}")))?;
        let usage = tx
            .query_row(
                "select job_id, steps, wall_ms, spend_usd, tool_calls, updated_ms
                 from queue_job_loop_usage where job_id = ?1",
                params![job_id],
                read_loop_usage_row,
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to read loop usage after update: {err}"))
            })?;
        let limits = tx
            .query_row(
                "select job_id, max_steps, max_wall_ms, max_spend_usd, max_tool_calls, updated_ms
                 from queue_job_loop_limits where job_id = ?1",
                params![job_id],
                read_loop_limits_row,
            )
            .optional()
            .map_err(|err| {
                RuntimeError::Other(format!("failed to read loop limits after update: {err}"))
            })?;
        let violated_bounds = limits
            .as_ref()
            .map(|limits| loop_bound_violations(&usage, limits))
            .unwrap_or_default();
        if !violated_bounds.is_empty() {
            let reason = violated_bounds.join(",");
            tx.execute(
                "update queue_jobs
                 set status = 'loop_budget_exceeded',
                     failure_kind = 'loop_bound_exceeded',
                     failure_fingerprint = ?2,
                     next_run_ms = null,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?3
                 where id = ?1",
                params![job_id, reason, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to stop loop-bound job: {err}")))?;
            insert_job_audit_event(
                &tx,
                job_id,
                "loop_bound_exceeded",
                &actor,
                None,
                &status_before,
                QueueJobStatus::LoopBudgetExceeded.as_str(),
                Some(&reason),
                now,
            )?;
        }
        tx.commit()
            .map_err(|err| RuntimeError::Other(format!("failed to commit loop usage: {err}")))?;
        Ok(JobLoopUsageReport {
            usage,
            violated_bounds,
        })
    }

    pub fn set_stall_policy(
        &self,
        job_id: &str,
        stall_after_ms: u64,
        action: JobStallAction,
    ) -> Result<JobStallPolicy, RuntimeError> {
        if stall_after_ms == 0 {
            return Err(RuntimeError::Other(
                "std.queue stall threshold must be greater than zero".to_string(),
            ));
        }
        if self.get(job_id)?.is_none() {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{job_id}` not found"
            )));
        }
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_job_stall_policies
                 (job_id, stall_after_ms, action, updated_ms)
                 values (?1, ?2, ?3, ?4)
                 on conflict(job_id) do update set
                    stall_after_ms = excluded.stall_after_ms,
                    action = excluded.action,
                    updated_ms = excluded.updated_ms",
                params![job_id, stall_after_ms as i64, action.as_str(), now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to set stall policy: {err}")))?;
        self.stall_policy(job_id)?.ok_or_else(|| {
            RuntimeError::Other(format!("std.queue stall policy for `{job_id}` not found"))
        })
    }

    pub fn stall_policy(&self, job_id: &str) -> Result<Option<JobStallPolicy>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select job_id, stall_after_ms, action, updated_ms
                 from queue_job_stall_policies where job_id = ?1",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to prepare stall policy read: {err}"))
            })?;
        let mut rows = stmt
            .query(params![job_id])
            .map_err(|err| RuntimeError::Other(format!("failed to query stall policy: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read stall policy row: {err}")))?
        {
            Ok(Some(read_stall_policy_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode stall policy: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn record_loop_heartbeat(
        &self,
        job_id: &str,
        actor: impl Into<String>,
        message: Option<String>,
    ) -> Result<JobLoopHeartbeat, RuntimeError> {
        self.record_loop_heartbeat_at(job_id, actor, message, now_ms())
    }

    pub fn record_loop_heartbeat_at(
        &self,
        job_id: &str,
        actor: impl Into<String>,
        message: Option<String>,
        now: u64,
    ) -> Result<JobLoopHeartbeat, RuntimeError> {
        let actor = actor.into();
        if actor.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue loop heartbeat actor must not be empty".to_string(),
            ));
        }
        if self.get(job_id)?.is_none() {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{job_id}` not found"
            )));
        }
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_job_loop_heartbeats
                 (job_id, actor, message, last_heartbeat_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5)
                 on conflict(job_id) do update set
                    actor = excluded.actor,
                    message = excluded.message,
                    last_heartbeat_ms = excluded.last_heartbeat_ms,
                    updated_ms = excluded.updated_ms",
                params![job_id, actor, message, now as i64, now as i64],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to record loop heartbeat: {err}"))
            })?;
        self.loop_heartbeat(job_id)?.ok_or_else(|| {
            RuntimeError::Other(format!("std.queue heartbeat for `{job_id}` not found"))
        })
    }

    pub fn loop_heartbeat(&self, job_id: &str) -> Result<Option<JobLoopHeartbeat>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select job_id, actor, message, last_heartbeat_ms, updated_ms
                 from queue_job_loop_heartbeats where job_id = ?1",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to prepare heartbeat read: {err}"))
            })?;
        let mut rows = stmt
            .query(params![job_id])
            .map_err(|err| RuntimeError::Other(format!("failed to query loop heartbeat: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read heartbeat row: {err}")))?
        {
            Ok(Some(read_loop_heartbeat_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode loop heartbeat: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn check_stall(
        &self,
        job_id: &str,
        actor: impl Into<String>,
    ) -> Result<JobStallCheck, RuntimeError> {
        self.check_stall_at(job_id, actor, now_ms())
    }

    pub fn check_stall_at(
        &self,
        job_id: &str,
        actor: impl Into<String>,
        now: u64,
    ) -> Result<JobStallCheck, RuntimeError> {
        let actor = actor.into();
        if actor.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue stall checker actor must not be empty".to_string(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!("failed to start stall check transaction: {err}"))
        })?;
        let (status_before, job_updated_ms) = tx
            .query_row(
                "select status, updated_ms from queue_jobs where id = ?1",
                params![job_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
            )
            .map_err(|err| {
                RuntimeError::Other(format!(
                    "std.queue job `{job_id}` not found or unreadable: {err}"
                ))
            })?;
        let policy = tx
            .query_row(
                "select job_id, stall_after_ms, action, updated_ms
                 from queue_job_stall_policies where job_id = ?1",
                params![job_id],
                read_stall_policy_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read stall policy: {err}")))?
            .ok_or_else(|| {
                RuntimeError::Other(format!("std.queue job `{job_id}` has no stall policy"))
            })?;
        let heartbeat = tx
            .query_row(
                "select job_id, actor, message, last_heartbeat_ms, updated_ms
                 from queue_job_loop_heartbeats where job_id = ?1",
                params![job_id],
                read_loop_heartbeat_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read loop heartbeat: {err}")))?;
        let last_heartbeat_ms = heartbeat
            .as_ref()
            .map(|heartbeat| heartbeat.last_heartbeat_ms)
            .unwrap_or(job_updated_ms);
        let elapsed_ms = now.saturating_sub(last_heartbeat_ms);
        let already_terminal = matches!(
            parse_status(&status_before),
            QueueJobStatus::Succeeded
                | QueueJobStatus::DeadLettered
                | QueueJobStatus::Canceled
                | QueueJobStatus::ApprovalDenied
                | QueueJobStatus::ApprovalExpired
                | QueueJobStatus::LoopBudgetExceeded
                | QueueJobStatus::LoopStallTerminated
        );
        let stalled = elapsed_ms > policy.stall_after_ms && !already_terminal;
        let mut action_taken = None;
        if stalled {
            let status_after = match policy.action {
                JobStallAction::Escalate => QueueJobStatus::LoopStallEscalated,
                JobStallAction::Terminate => QueueJobStatus::LoopStallTerminated,
            };
            let reason = format!(
                "loop_stalled:last_heartbeat_ms={last_heartbeat_ms},elapsed_ms={elapsed_ms},stall_after_ms={}",
                policy.stall_after_ms
            );
            tx.execute(
                "update queue_jobs
                 set status = ?2,
                     failure_kind = 'loop_stalled',
                     failure_fingerprint = ?3,
                     next_run_ms = null,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?4
                 where id = ?1",
                params![job_id, status_after.as_str(), reason, now as i64],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to apply stall transition: {err}"))
            })?;
            let event_kind = format!("loop_stalled_{}", policy.action.as_str());
            insert_job_audit_event(
                &tx,
                job_id,
                &event_kind,
                &actor,
                None,
                &status_before,
                status_after.as_str(),
                Some(&reason),
                now,
            )?;
            action_taken = Some(policy.action.as_str().to_string());
        }
        tx.commit()
            .map_err(|err| RuntimeError::Other(format!("failed to commit stall check: {err}")))?;
        Ok(JobStallCheck {
            job_id: job_id.to_string(),
            stalled,
            action_taken,
            last_heartbeat_ms,
            stall_after_ms: policy.stall_after_ms,
            elapsed_ms,
        })
    }
}

fn loop_bound_violations(usage: &JobLoopUsage, limits: &JobLoopLimits) -> Vec<String> {
    let mut violations = Vec::new();
    if limits.max_steps.is_some_and(|limit| usage.steps > limit) {
        violations.push(format!(
            "max_steps:{}>{}",
            usage.steps,
            limits.max_steps.unwrap()
        ));
    }
    if limits
        .max_wall_ms
        .is_some_and(|limit| usage.wall_ms > limit)
    {
        violations.push(format!(
            "max_wall_ms:{}>{}",
            usage.wall_ms,
            limits.max_wall_ms.unwrap()
        ));
    }
    if limits
        .max_spend_usd
        .is_some_and(|limit| usage.spend_usd > limit)
    {
        violations.push(format!(
            "max_spend_usd:{:.6}>{:.6}",
            usage.spend_usd,
            limits.max_spend_usd.unwrap()
        ));
    }
    if limits
        .max_tool_calls
        .is_some_and(|limit| usage.tool_calls > limit)
    {
        violations.push(format!(
            "max_tool_calls:{}>{}",
            usage.tool_calls,
            limits.max_tool_calls.unwrap()
        ));
    }
    violations
}
