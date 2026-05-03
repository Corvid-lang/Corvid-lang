use super::*;

impl DurableQueueRuntime {
    pub fn upsert_schedule(
        &self,
        manifest: QueueScheduleManifest,
    ) -> Result<QueueScheduleManifest, RuntimeError> {
        validate_schedule(&manifest.cron, &manifest.zone)?;
        if manifest.id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue schedule id must not be empty".to_string(),
            ));
        }
        if manifest.task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue schedule task must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let payload_json = serde_json::to_string(&manifest.payload).map_err(|err| {
            RuntimeError::Other(format!(
                "failed to serialize durable schedule payload: {err}"
            ))
        })?;
        let budget_usd = if manifest.budget_usd.is_finite() && manifest.budget_usd > 0.0 {
            manifest.budget_usd
        } else {
            0.0
        };
        let created_ms = if manifest.created_ms == 0 {
            now
        } else {
            manifest.created_ms
        };
        let last_checked_ms = if manifest.last_checked_ms == 0 {
            now
        } else {
            manifest.last_checked_ms
        };
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_schedules
                 (id, cron, zone, task, payload_json, max_retries, budget_usd, effect_summary, replay_key_prefix, missed_policy, last_checked_ms, last_fire_ms, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 on conflict(id) do update set
                   cron = excluded.cron,
                   zone = excluded.zone,
                   task = excluded.task,
                   payload_json = excluded.payload_json,
                   max_retries = excluded.max_retries,
                   budget_usd = excluded.budget_usd,
                   effect_summary = excluded.effect_summary,
                   replay_key_prefix = excluded.replay_key_prefix,
                   missed_policy = excluded.missed_policy,
                   updated_ms = excluded.updated_ms",
                params![
                    manifest.id,
                    manifest.cron,
                    manifest.zone,
                    manifest.task,
                    payload_json,
                    manifest.max_retries as i64,
                    budget_usd,
                    manifest.effect_summary,
                    manifest.replay_key_prefix,
                    manifest.missed_policy.as_str(),
                    last_checked_ms as i64,
                    manifest.last_fire_ms.map(|value| value as i64),
                    created_ms as i64,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to upsert schedule: {err}")))?;
        self.get_schedule(&manifest.id)?.ok_or_else(|| {
            RuntimeError::Other(format!("schedule `{}` not found after upsert", manifest.id))
        })
    }

    pub fn get_schedule(&self, id: &str) -> Result<Option<QueueScheduleManifest>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, cron, zone, task, payload_json, max_retries, budget_usd, effect_summary, replay_key_prefix, missed_policy, last_checked_ms, last_fire_ms, created_ms, updated_ms
                 from queue_schedules where id = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare schedule read: {err}")))?;
        let mut rows = stmt
            .query(params![id])
            .map_err(|err| RuntimeError::Other(format!("failed to query schedule: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read schedule row: {err}")))?
        {
            Ok(Some(read_schedule_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode schedule: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn list_schedules(&self) -> Result<Vec<QueueScheduleManifest>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, cron, zone, task, payload_json, max_retries, budget_usd, effect_summary, replay_key_prefix, missed_policy, last_checked_ms, last_fire_ms, created_ms, updated_ms
                 from queue_schedules order by id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare schedule list: {err}")))?;
        let rows = stmt
            .query_map([], read_schedule_row)
            .map_err(|err| RuntimeError::Other(format!("failed to list schedules: {err}")))?;
        let mut schedules = Vec::new();
        for row in rows {
            schedules.push(
                row.map_err(|err| {
                    RuntimeError::Other(format!("failed to decode schedule: {err}"))
                })?,
            );
        }
        Ok(schedules)
    }

    pub fn recover_schedules(
        &self,
        max_missed_per_schedule: usize,
    ) -> Result<SchedulerRecoveryReport, RuntimeError> {
        self.recover_schedules_at(now_ms(), max_missed_per_schedule)
    }

    pub fn recover_schedules_at(
        &self,
        now: u64,
        max_missed_per_schedule: usize,
    ) -> Result<SchedulerRecoveryReport, RuntimeError> {
        let schedules = self.list_schedules()?;
        let mut report = SchedulerRecoveryReport {
            scanned: schedules.len(),
            ..SchedulerRecoveryReport::default()
        };
        for schedule in schedules {
            let due = missed_fire_times(&schedule, now, max_missed_per_schedule)?;
            if due.is_empty() {
                self.mark_schedule_checked(&schedule.id, now, schedule.last_fire_ms)?;
                continue;
            }
            match schedule.missed_policy {
                ScheduleMissedPolicy::SkipMissed => {
                    report.skipped = report.skipped.saturating_add(due.len());
                    for fire_ms in &due {
                        report.recoveries.push(ScheduleRecovery {
                            schedule_id: schedule.id.clone(),
                            task: schedule.task.clone(),
                            fire_ms: *fire_ms,
                            job_id: None,
                            policy: schedule.missed_policy,
                            action: "skipped".to_string(),
                        });
                    }
                    self.mark_schedule_checked(&schedule.id, now, due.last().copied())?;
                }
                ScheduleMissedPolicy::FireOnceOnRecovery => {
                    let Some(&fire_ms) = due.last() else { continue };
                    if let Some(job) = self.enqueue_schedule_fire(&schedule, fire_ms)? {
                        report.enqueued = report.enqueued.saturating_add(1);
                        report.recoveries.push(ScheduleRecovery {
                            schedule_id: schedule.id.clone(),
                            task: schedule.task.clone(),
                            fire_ms,
                            job_id: Some(job.id),
                            policy: schedule.missed_policy,
                            action: "enqueued".to_string(),
                        });
                    }
                    self.mark_schedule_checked(&schedule.id, now, Some(fire_ms))?;
                }
                ScheduleMissedPolicy::EnqueueAllBounded => {
                    let mut last_fire = schedule.last_fire_ms;
                    for fire_ms in due {
                        last_fire = Some(fire_ms);
                        if let Some(job) = self.enqueue_schedule_fire(&schedule, fire_ms)? {
                            report.enqueued = report.enqueued.saturating_add(1);
                            report.recoveries.push(ScheduleRecovery {
                                schedule_id: schedule.id.clone(),
                                task: schedule.task.clone(),
                                fire_ms,
                                job_id: Some(job.id),
                                policy: schedule.missed_policy,
                                action: "enqueued".to_string(),
                            });
                        }
                    }
                    self.mark_schedule_checked(&schedule.id, now, last_fire)?;
                }
            }
        }
        Ok(report)
    }

    fn enqueue_schedule_fire(
        &self,
        schedule: &QueueScheduleManifest,
        fire_ms: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let event_id = format!("{}:{fire_ms}", schedule.id);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!(
                "failed to start schedule recovery transaction: {err}"
            ))
        })?;
        let inserted = tx
            .execute(
                "insert or ignore into queue_schedule_fires (event_id, schedule_id, fire_ms, job_id, created_ms)
                 values (?1, ?2, ?3, '', ?4)",
                params![event_id, schedule.id, fire_ms as i64, now_ms() as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to record schedule fire: {err}")))?;
        if inserted == 0 {
            tx.commit().map_err(|err| {
                RuntimeError::Other(format!(
                    "failed to commit duplicate schedule fire record: {err}"
                ))
            })?;
            return Ok(None);
        }
        let mut payload = serde_json::Map::new();
        payload.insert(
            "schedule_id".to_string(),
            Value::String(schedule.id.clone()),
        );
        payload.insert(
            "scheduled_fire_ms".to_string(),
            Value::Number(serde_json::Number::from(fire_ms)),
        );
        payload.insert("payload".to_string(), schedule.payload.clone());
        tx.commit().map_err(|err| {
            RuntimeError::Other(format!("failed to commit schedule fire record: {err}"))
        })?;
        drop(conn);
        let replay_key = schedule
            .replay_key_prefix
            .as_ref()
            .map(|prefix| format!("{prefix}:{fire_ms}"));
        let job = self.enqueue_typed(
            schedule.task.clone(),
            Value::Object(payload),
            None,
            schedule.max_retries,
            schedule.budget_usd,
            schedule.effect_summary.clone(),
            replay_key,
        )?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update queue_schedule_fires set job_id = ?2 where event_id = ?1",
                params![event_id, job.id],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to link schedule fire to job: {err}"))
            })?;
        Ok(Some(job))
    }

    fn mark_schedule_checked(
        &self,
        schedule_id: &str,
        checked_ms: u64,
        last_fire_ms: Option<u64>,
    ) -> Result<(), RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update queue_schedules set last_checked_ms = ?2, last_fire_ms = coalesce(?3, last_fire_ms), updated_ms = ?2 where id = ?1",
                params![schedule_id, checked_ms as i64, last_fire_ms.map(|value| value as i64)],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to update schedule recovery cursor: {err}")))?;
        Ok(())
    }
}
