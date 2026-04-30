//! Cron-expression + timezone helpers for the durable scheduler
//! — slice 38M / DST-aware cron, decomposed in Phase 20j-A2.
//!
//! Three responsibilities:
//!
//! - [`validate_schedule`] checks a cron expression + timezone
//!   round-trip against the parser before persisting them.
//! - [`missed_fire_times`] iterates `cron.after(..)` to compute
//!   the missed UTC fire moments between a schedule's last
//!   check + now, capped by `max_missed_per_schedule`. The
//!   `chrono-tz` zone is consulted so DST transitions resolve
//!   to the documented policy.
//! - [`normalize_cron`] accepts 5/6/7-field cron expressions
//!   and projects them onto the `cron` crate's required
//!   second-resolution form.

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::str::FromStr;

use super::model::QueueScheduleManifest;
use crate::errors::RuntimeError;

pub(super) fn validate_schedule(cron: &str, zone: &str) -> Result<(), RuntimeError> {
    let expression = normalize_cron(cron)?;
    Schedule::from_str(&expression)
        .map_err(|err| RuntimeError::Other(format!("invalid cron expression `{cron}`: {err}")))?;
    zone.parse::<Tz>()
        .map_err(|err| RuntimeError::Other(format!("invalid schedule timezone `{zone}`: {err}")))?;
    Ok(())
}

pub(super) fn missed_fire_times(
    schedule: &QueueScheduleManifest,
    now: u64,
    max_missed_per_schedule: usize,
) -> Result<Vec<u64>, RuntimeError> {
    if now <= schedule.last_checked_ms || max_missed_per_schedule == 0 {
        return Ok(Vec::new());
    }
    let expression = normalize_cron(&schedule.cron)?;
    let cron = Schedule::from_str(&expression).map_err(|err| {
        RuntimeError::Other(format!(
            "invalid cron expression `{}`: {err}",
            schedule.cron
        ))
    })?;
    let zone = schedule.zone.parse::<Tz>().map_err(|err| {
        RuntimeError::Other(format!(
            "invalid schedule timezone `{}`: {err}",
            schedule.zone
        ))
    })?;
    let start_ms = schedule
        .last_fire_ms
        .unwrap_or(schedule.last_checked_ms)
        .saturating_add(1);
    let start = Utc
        .timestamp_millis_opt(start_ms as i64)
        .single()
        .ok_or_else(|| {
            RuntimeError::Other(format!("invalid schedule recovery start `{start_ms}`"))
        })?;
    let end = Utc
        .timestamp_millis_opt(now as i64)
        .single()
        .ok_or_else(|| RuntimeError::Other(format!("invalid schedule recovery end `{now}`")))?;
    let start_local = start.with_timezone(&zone);
    let end_local = end.with_timezone(&zone);
    let mut fires = Vec::new();
    for fire in cron.after(&start_local).take(max_missed_per_schedule) {
        if fire > end_local {
            break;
        }
        fires.push(fire.with_timezone(&Utc).timestamp_millis() as u64);
    }
    Ok(fires)
}

pub(super) fn normalize_cron(cron: &str) -> Result<String, RuntimeError> {
    let fields = cron.split_whitespace().collect::<Vec<_>>();
    match fields.len() {
        5 => Ok(format!(
            "0 {} {} {} {} {} *",
            fields[0], fields[1], fields[2], fields[3], fields[4]
        )),
        6 => Ok(format!(
            "{} {} {} {} {} {} *",
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[5]
        )),
        7 => Ok(cron.to_string()),
        _ => Err(RuntimeError::Other(format!(
            "invalid cron expression `{cron}`: expected 5, 6, or 7 fields"
        ))),
    }
}
