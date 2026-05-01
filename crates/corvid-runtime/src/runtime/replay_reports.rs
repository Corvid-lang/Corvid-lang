//! Replay-mode predicates, calibration recording, replay-report
//! writers, LLM-usage / observation summaries, and the
//! `prepare_run` / `complete_run` lifecycle hooks the interpreter
//! invokes around each agent invocation.
//!
//! Live runs flow through the same surface — the predicates
//! return their no-op answers, calibration writes go to the
//! durable store, and `prepare_run` / `complete_run` are no-ops.
//! Replay-mode runs additionally consult the `ReplaySource`
//! attached to the `Runtime` for mutation / differential reports.

use std::path::Path;

use crate::calibration::CalibrationStats;
use crate::errors::RuntimeError;
use crate::observe::{
    provider_observations, runtime_observation_summary, ProviderObservation,
    RuntimeObservationSummary,
};
use crate::record::Recorder;
use crate::replay::{ReplayDifferentialReport, ReplayMutationReport, ReplaySource};
use crate::usage::{LlmUsageRecord, LlmUsageTotals};

use super::{Runtime, RuntimeMode};

impl Runtime {
    pub fn recorder(&self) -> Option<&Recorder> {
        self.recorder.as_deref()
    }

    pub fn is_replay_mode(&self) -> bool {
        matches!(self.mode, RuntimeMode::Replay(_))
    }

    pub fn replay_uses_live_llm(&self) -> bool {
        matches!(&self.mode, RuntimeMode::Replay(source) if source.uses_live_llm())
    }

    pub fn record_calibration(
        &self,
        prompt: &str,
        model: &str,
        confidence: f64,
        actual_correct: bool,
    ) {
        self.calibration
            .record(prompt, model, confidence, actual_correct);
    }

    pub fn calibration_stats(&self, prompt: &str, model: &str) -> Option<CalibrationStats> {
        self.calibration.stats(prompt, model)
    }

    pub fn replay_differential_report(&self) -> Option<ReplayDifferentialReport> {
        match &self.mode {
            RuntimeMode::Live => None,
            RuntimeMode::Replay(source) => source.differential_report(),
        }
    }

    pub fn replay_mutation_report(&self) -> Option<ReplayMutationReport> {
        match &self.mode {
            RuntimeMode::Live => None,
            RuntimeMode::Replay(source) => source.mutation_report(),
        }
    }

    pub fn write_replay_differential_report(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), RuntimeError> {
        let path = path.as_ref();
        let Some(report) = self.replay_differential_report() else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&report).map_err(|err| {
            RuntimeError::Other(format!(
                "failed to serialize replay differential report: {err}"
            ))
        })?;
        std::fs::write(path, bytes).map_err(|err| {
            RuntimeError::Other(format!(
                "failed to write replay differential report to `{}`: {err}",
                path.display()
            ))
        })
    }

    pub fn write_replay_mutation_report(&self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        let path = path.as_ref();
        let Some(report) = self.replay_mutation_report() else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&report).map_err(|err| {
            RuntimeError::Other(format!("failed to serialize replay mutation report: {err}"))
        })?;
        std::fs::write(path, bytes).map_err(|err| {
            RuntimeError::Other(format!(
                "failed to write replay mutation report to `{}`: {err}",
                path.display()
            ))
        })
    }

    pub fn llm_usage_records(&self) -> Vec<LlmUsageRecord> {
        self.usage_ledger.records()
    }

    pub fn llm_usage_totals_by_provider(&self) -> std::collections::BTreeMap<String, LlmUsageTotals> {
        self.usage_ledger.totals_by_provider()
    }

    pub fn observation_summary(&self) -> RuntimeObservationSummary {
        let usage = self.llm_usage_records();
        let health = self.provider_health();
        runtime_observation_summary(&usage, &health)
    }

    pub fn provider_observations(&self) -> Vec<ProviderObservation> {
        provider_observations(&self.provider_health())
    }

    pub fn emit_observation_summary(&self) -> RuntimeObservationSummary {
        let summary = self.observation_summary();
        self.emit_host_event(
            "std.observe.summary",
            serde_json::json!({
                "llm_calls": summary.llm_calls,
                "local_llm_calls": summary.local_llm_calls,
                "prompt_tokens": summary.prompt_tokens,
                "completion_tokens": summary.completion_tokens,
                "total_tokens": summary.total_tokens,
                "cost_usd": summary.cost_usd,
                "currency": "USD",
                "provider_count": summary.provider_count,
                "degraded_provider_count": summary.degraded_provider_count,
            }),
        );
        summary
    }

    pub fn prepare_run(&self, agent: &str, args: &[serde_json::Value]) -> Result<(), RuntimeError> {
        if let Some(replay) = self.replay_source()? {
            replay.prepare_run(agent, args)?;
        }
        Ok(())
    }

    pub fn complete_run(
        &self,
        ok: bool,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> Result<(), RuntimeError> {
        if let Some(replay) = self.replay_source()? {
            replay.complete_run(ok, result, error)?;
        }
        Ok(())
    }

    pub(super) fn replay_source(&self) -> Result<Option<&ReplaySource>, RuntimeError> {
        if let Some(err) = &self.replay_error {
            return Err(err.clone());
        }
        Ok(match &self.mode {
            RuntimeMode::Live => None,
            RuntimeMode::Replay(source) => Some(source),
        })
    }
}
