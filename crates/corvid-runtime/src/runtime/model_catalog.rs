//! Model catalog and provider-health methods on `Runtime`.
//!
//! These answer "what model should I use?" / "is the provider
//! healthy?" / "what version is this model?" queries that callers
//! need before invoking `call_llm`. Cheapest-by-capability and
//! cheapest-by-requirements selection consult the runtime's
//! `ModelCatalog`; provider health flows through the
//! `LlmRegistry`. `choose_rollout_variant` lives here too because
//! its rollout-sample seed feeds the same model-selection layer
//! (and is replay-deterministic via `next_rollout_sample`).

use std::sync::atomic::Ordering;

use crate::capability_contract::{
    run_capability_contracts, CapabilityContractOptions, CapabilityContractReport,
};
use crate::errors::RuntimeError;
use crate::llm::ProviderHealth;
use crate::models::{ModelCatalog, ModelSelection};

use super::Runtime;

impl Runtime {
    pub fn model_catalog(&self) -> &ModelCatalog {
        &self.model_catalog
    }

    pub fn provider_health(&self) -> Vec<ProviderHealth> {
        self.llms.health()
    }

    pub async fn check_model_capability_contracts(
        &self,
        options: CapabilityContractOptions,
    ) -> Result<CapabilityContractReport, RuntimeError> {
        run_capability_contracts(self, options).await
    }

    pub fn select_cheapest_model_for_capability(
        &self,
        required_capability: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Result<ModelSelection, RuntimeError> {
        if let Some(err) = &self.model_catalog_error {
            return Err(err.clone());
        }
        self.model_catalog.select_cheapest_by_capability(
            required_capability,
            prompt_tokens,
            completion_tokens,
        )
    }

    pub fn select_cheapest_model_for_requirements(
        &self,
        required_capability: Option<&str>,
        required_output_format: Option<&str>,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Result<ModelSelection, RuntimeError> {
        if let Some(err) = &self.model_catalog_error {
            return Err(err.clone());
        }
        self.model_catalog.select_cheapest_by_requirements(
            required_capability,
            required_output_format,
            prompt_tokens,
            completion_tokens,
        )
    }

    pub fn describe_named_model(
        &self,
        model_name: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Result<ModelSelection, RuntimeError> {
        if let Some(err) = &self.model_catalog_error {
            return Err(err.clone());
        }
        Ok(self
            .model_catalog
            .describe_named_model(model_name, prompt_tokens, completion_tokens))
    }

    pub fn model_version(&self, model_name: &str) -> Option<String> {
        if model_name.is_empty() {
            return None;
        }
        self.model_catalog
            .get(model_name)
            .and_then(|model| model.version.clone())
    }

    pub fn choose_rollout_variant(&self, variant_percent: f64) -> Result<bool, RuntimeError> {
        if variant_percent <= 0.0 {
            return Ok(false);
        }
        if variant_percent >= 100.0 {
            return Ok(true);
        }
        self.next_rollout_sample()
            .map(|sample| sample < (variant_percent / 100.0))
    }

    fn next_rollout_sample(&self) -> Result<f64, RuntimeError> {
        let next = if let Some(replay) = self.replay_source()? {
            let next = replay.replay_rollout_sample()?;
            self.rollout_state.store(next, Ordering::SeqCst);
            next
        } else {
            loop {
                let current = self.rollout_state.load(Ordering::Relaxed);
                let next = current
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                if self
                    .rollout_state
                    .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break next;
                }
            }
        };
        if let Some(recorder) = &self.recorder {
            recorder.emit_seed_read("rollout_cohort", next);
        }
        let mantissa = next >> 11;
        Ok(mantissa as f64 / ((1_u64 << 53) as f64))
    }
}
