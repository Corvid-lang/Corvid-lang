//! `Runtime` — the top-level glue the interpreter calls into.
//!
//! Bundles the tool registry, LLM registry, approver, and tracer behind
//! one handle. Construct with `Runtime::builder()`, populate with tools
//! and adapters, freeze with `.build()`. Pass `&Runtime` to the
//! interpreter.

use crate::approvals::{
    ApprovalDecision, ApprovalRequest, ApprovalToken, Approver, StdinApprover,
};
use crate::calibration::{CalibrationStats, CalibrationStore};
use crate::errors::RuntimeError;
use crate::human::{HumanChoiceRequest, HumanInputRequest, HumanInteractor, StdinHumanInteractor};
use crate::llm::{LlmAdapter, LlmRegistry, LlmRequest, LlmRequestRef, LlmResponse};
use crate::models::{ModelCatalog, ModelSelection, RegisteredModel};
use crate::prompt_cache::PromptCache;
use crate::record::Recorder;
use crate::replay::{ReplayDifferentialReport, ReplayMutationReport, ReplaySource};
use crate::tools::ToolRegistry;
use crate::tracing::{fresh_run_id, now_ms, Tracer};
use corvid_trace_schema::{TraceEvent, WRITER_INTERPRETER};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const APPROVAL_TOKEN_SCOPE_ONE_TIME: &str = "one_time";
const APPROVAL_TOKEN_TTL_MS: u64 = 5 * 60 * 1000;

#[derive(Clone)]
pub struct Runtime {
    tools: ToolRegistry,
    llms: LlmRegistry,
    approver: Arc<dyn Approver>,
    human: Arc<dyn HumanInteractor>,
    tracer: Tracer,
    recorder: Option<Arc<Recorder>>,
    mode: RuntimeMode,
    replay_error: Option<RuntimeError>,
    /// Default model name applied when a prompt call doesn't specify one.
    /// Empty string means "no default; require per-call override".
    default_model: String,
    model_catalog: ModelCatalog,
    model_catalog_error: Option<RuntimeError>,
    rollout_state: Arc<AtomicU64>,
    calibration: CalibrationStore,
    prompt_cache: PromptCache,
}

#[derive(Clone)]
enum RuntimeMode {
    Live,
    Replay(ReplaySource),
}

impl Runtime {
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    // ---- accessors used by the interpreter ----

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn tracer(&self) -> &Tracer {
        &self.tracer
    }

    pub fn recorder(&self) -> Option<&Recorder> {
        self.recorder.as_deref()
    }

    pub fn is_replay_mode(&self) -> bool {
        matches!(self.mode, RuntimeMode::Replay(_))
    }

    pub fn replay_uses_live_llm(&self) -> bool {
        matches!(&self.mode, RuntimeMode::Replay(source) if source.uses_live_llm())
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
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

    pub fn model_catalog(&self) -> &ModelCatalog {
        &self.model_catalog
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

    fn replay_source(&self) -> Result<Option<&ReplaySource>, RuntimeError> {
        if let Some(err) = &self.replay_error {
            return Err(err.clone());
        }
        Ok(match &self.mode {
            RuntimeMode::Live => None,
            RuntimeMode::Replay(source) => Some(source),
        })
    }

    // ---- dispatch helpers ----

    /// Call a tool by name. Emits trace events bracketing the call.
    pub async fn call_tool(
        &self,
        name: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, RuntimeError> {
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::ToolCall {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                tool: name.to_string(),
                args: args.clone(),
            });
        }
        let result = if let Some(replay) = self.replay_source()? {
            replay.replay_tool_call(name, &args)?
        } else {
            self.tools.call(name, args.clone()).await?
        };
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::ToolResult {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                tool: name.to_string(),
                result: result.clone(),
            });
        }
        Ok(result)
    }

    /// Call an LLM. Falls back to `default_model` if `req.model` is empty.
    pub async fn call_llm(&self, mut req: LlmRequest) -> Result<LlmResponse, RuntimeError> {
        if req.model.is_empty() {
            req.model = self.default_model.clone();
        }
        self.call_llm_ref(req.as_ref()).await
    }

    /// Call an LLM through the prompt-response cache when the source
    /// prompt declared `cacheable: true`. Replay mode bypasses the live
    /// cache and consumes the recorded `LlmCall` / `LlmResult` pair instead.
    pub async fn call_llm_cacheable(
        &self,
        mut req: LlmRequest,
        cacheable: bool,
    ) -> Result<LlmResponse, RuntimeError> {
        if req.model.is_empty() {
            req.model = self.default_model.clone();
        }
        self.call_llm_ref_impl(req.as_ref(), None, cacheable).await
    }

    pub async fn call_llm_ref_with_trace_rendered(
        &self,
        req: LlmRequestRef<'_>,
        trace_rendered: Option<&str>,
    ) -> Result<LlmResponse, RuntimeError> {
        self.call_llm_ref_impl(req, trace_rendered, false).await
    }

    /// Borrowed LLM-call path for native bridges that already hold prompt and
    /// rendered text as borrowed strings and only need owned clones when
    /// tracing or provider JSON construction requires them.
    pub async fn call_llm_ref(&self, req: LlmRequestRef<'_>) -> Result<LlmResponse, RuntimeError> {
        self.call_llm_ref_impl(req, None, false).await
    }

    async fn call_llm_ref_impl(
        &self,
        req: LlmRequestRef<'_>,
        trace_rendered_override: Option<&str>,
        cacheable: bool,
    ) -> Result<LlmResponse, RuntimeError> {
        let req = if req.model.is_empty() {
            req.with_model(&self.default_model)
        } else {
            req
        };
        let trace_rendered = trace_rendered_override.unwrap_or(req.rendered);
        let replay = self.replay_source()?;
        let live_model_override = replay
            .and_then(|source| source.live_model_override())
            .map(str::to_owned);
        let trace_model = live_model_override.as_deref().unwrap_or(req.model);
        let recorded_model_version = self.model_version(req.model);
        let trace_model_version = self.model_version(trace_model);
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::LlmCall {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                prompt: req.prompt.to_string(),
                model: if trace_model.is_empty() {
                    None
                } else {
                    Some(trace_model.to_string())
                },
                model_version: trace_model_version.clone(),
                rendered: Some(trace_rendered.to_string()),
                args: req.args.to_vec(),
            });
        }
        let cache_fingerprint = if cacheable && replay.is_none() {
            Some(PromptCache::fingerprint(req))
        } else {
            None
        };
        if let Some(fingerprint) = cache_fingerprint.as_deref() {
            if let Some(cached) = self.prompt_cache.get(fingerprint) {
                if self.tracer.is_enabled() {
                    self.tracer.emit(TraceEvent::PromptCache {
                        ts_ms: now_ms(),
                        run_id: self.tracer.run_id().to_string(),
                        prompt: req.prompt.to_string(),
                        model: if trace_model.is_empty() {
                            None
                        } else {
                            Some(trace_model.to_string())
                        },
                        model_version: trace_model_version.clone(),
                        fingerprint: fingerprint.to_string(),
                        hit: true,
                    });
                    self.tracer.emit(TraceEvent::LlmResult {
                        ts_ms: now_ms(),
                        run_id: self.tracer.run_id().to_string(),
                        prompt: req.prompt.to_string(),
                        model: if trace_model.is_empty() {
                            None
                        } else {
                            Some(trace_model.to_string())
                        },
                        model_version: trace_model_version.clone(),
                        result: cached.value.clone(),
                    });
                }
                return Ok(PromptCache::cached_response(cached));
            }
        }
        let resp = if let Some(replay) = replay {
            let live_req = if let Some(model) = live_model_override.as_deref() {
                req.with_model(model)
            } else {
                req
            };
            replay
                .replay_llm_call(
                    req.prompt,
                    if req.model.is_empty() {
                        None
                    } else {
                        Some(req.model)
                    },
                    recorded_model_version.as_deref(),
                    trace_rendered,
                    req.args,
                    live_req,
                    &self.llms,
                )
                .await?
        } else {
            self.llms.call(&req).await?
        };
        if let Some(fingerprint) = cache_fingerprint.as_deref() {
            self.prompt_cache
                .insert(fingerprint.to_string(), resp.clone());
            if self.tracer.is_enabled() {
                self.tracer.emit(TraceEvent::PromptCache {
                    ts_ms: now_ms(),
                    run_id: self.tracer.run_id().to_string(),
                    prompt: req.prompt.to_string(),
                    model: if trace_model.is_empty() {
                        None
                    } else {
                        Some(trace_model.to_string())
                    },
                    model_version: trace_model_version.clone(),
                    fingerprint: fingerprint.to_string(),
                    hit: false,
                });
            }
        }
        let actual_model = live_model_override.as_deref().unwrap_or(req.model);
        let cost_usd = if actual_model.is_empty() {
            0.0
        } else {
            self.model_catalog
                .describe_named_model(
                    actual_model,
                    resp.usage.prompt_tokens as u64,
                    resp.usage.completion_tokens as u64,
                )
                .cost_estimate
        };
        crate::observation_handles::record_llm_usage(resp.usage, cost_usd);
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::LlmResult {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                prompt: req.prompt.to_string(),
                model: if trace_model.is_empty() {
                    None
                } else {
                    Some(trace_model.to_string())
                },
                model_version: trace_model_version,
                result: resp.value.clone(),
            });
        }
        Ok(resp)
    }

    /// Ask the approver about an action. Returns `ApprovalDenied` if
    /// denied; the interpreter surfaces this as `InterpError::Runtime`.
    pub async fn approval_gate(
        &self,
        label: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<(), RuntimeError> {
        let trace_enabled = self.tracer.is_enabled();
        let label_owned = label.to_string();
        if trace_enabled {
            self.tracer.emit(TraceEvent::ApprovalRequest {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                label: label_owned.clone(),
                args: args.clone(),
            });
        }
        let req = ApprovalRequest {
            label: label_owned.clone(),
            args,
        };
        let (approved, detail) = if let Some(replay) = self.replay_source()? {
            let outcome = replay.replay_approval(&label_owned, &req.args)?;
            let detail =
                outcome
                    .decision
                    .map(|decision| crate::approver_bridge::ApprovalDecisionInfo {
                        accepted: decision.accepted,
                        decider: decision.decider,
                        rationale: decision.rationale,
                    });
            (outcome.approved, detail)
        } else {
            let approved = self.approver.approve(&req).await? == ApprovalDecision::Approve;
            let detail = Some(crate::catalog_c_api::take_last_approval_detail().unwrap_or(
                crate::approver_bridge::ApprovalDecisionInfo {
                    accepted: approved,
                    decider: "runtime-approver".to_string(),
                    rationale: None,
                },
            ));
            (approved, detail)
        };
        if trace_enabled {
            if let Some(detail) = detail {
                self.tracer.emit(TraceEvent::ApprovalDecision {
                    ts_ms: now_ms(),
                    run_id: self.tracer.run_id().to_string(),
                    site: label_owned.clone(),
                    args: req.args.clone(),
                    accepted: detail.accepted,
                    decider: detail.decider,
                    rationale: detail.rationale,
                });
            }
        }
        if trace_enabled {
            self.tracer.emit(TraceEvent::ApprovalResponse {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                label: label_owned.clone(),
                approved,
            });
        }
        if approved {
            if trace_enabled {
                let issued_at_ms = now_ms();
                let expires_at_ms = issued_at_ms.saturating_add(APPROVAL_TOKEN_TTL_MS);
                let run_id = self.tracer.run_id().to_string();
                self.tracer.emit(TraceEvent::ApprovalTokenIssued {
                    ts_ms: issued_at_ms,
                    run_id: run_id.clone(),
                    token_id: approval_token_id(
                        &run_id,
                        &label_owned,
                        &req.args,
                        APPROVAL_TOKEN_SCOPE_ONE_TIME,
                        issued_at_ms,
                        expires_at_ms,
                    ),
                    label: label_owned.clone(),
                    args: req.args.clone(),
                    scope: APPROVAL_TOKEN_SCOPE_ONE_TIME.to_string(),
                    issued_at_ms,
                    expires_at_ms,
                });
            }
            Ok(())
        } else {
            Err(RuntimeError::ApprovalDenied {
                action: label_owned,
            })
        }
    }

    pub fn validate_approval_token_scope(
        &self,
        token: &mut ApprovalToken,
        label: &str,
        args: &[serde_json::Value],
        session_id: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let now = now_ms();
        match token.validate(label, args, now, session_id) {
            Ok(()) => Ok(()),
            Err(reason) => {
                if self.tracer.is_enabled() {
                    self.tracer.emit(TraceEvent::ApprovalScopeViolation {
                        ts_ms: now,
                        run_id: self.tracer.run_id().to_string(),
                        token_id: token.token_id.clone(),
                        label: label.to_string(),
                        reason: reason.clone(),
                    });
                }
                Err(RuntimeError::ApprovalFailed(format!(
                    "approval token scope violation: {reason}"
                )))
            }
        }
    }

    pub async fn ask_human(
        &self,
        prompt: &str,
        expected_type: impl Into<String>,
    ) -> Result<serde_json::Value, RuntimeError> {
        let req = HumanInputRequest {
            prompt: prompt.to_string(),
            expected_type: expected_type.into(),
        };
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::HumanInputRequest {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                prompt: req.prompt.clone(),
                expected_type: req.expected_type.clone(),
            });
        }
        let value = self.human.ask(&req).await?;
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::HumanInputResponse {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                prompt: req.prompt,
                value: value.clone(),
            });
        }
        Ok(value)
    }

    pub async fn choose_human(
        &self,
        options: Vec<serde_json::Value>,
    ) -> Result<usize, RuntimeError> {
        let req = HumanChoiceRequest { options };
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::HumanChoiceRequest {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                options: req.options.clone(),
            });
        }
        let selected_index = self.human.choose(&req).await?;
        let selected_value = req.options.get(selected_index).cloned().ok_or_else(|| {
            RuntimeError::Other(format!("human choice index {selected_index} out of range"))
        })?;
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::HumanChoiceResponse {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                selected_index,
                selected_value,
            });
        }
        Ok(selected_index)
    }
}

fn approval_token_id(
    run_id: &str,
    label: &str,
    args: &[serde_json::Value],
    scope: &str,
    issued_at_ms: u64,
    expires_at_ms: u64,
) -> String {
    let args_json = serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string());
    let mut hasher = Sha256::new();
    hasher.update(run_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(label.as_bytes());
    hasher.update(b"\0");
    hasher.update(args_json.as_bytes());
    hasher.update(b"\0");
    hasher.update(scope.as_bytes());
    hasher.update(b"\0");
    hasher.update(issued_at_ms.to_le_bytes());
    hasher.update(expires_at_ms.to_le_bytes());
    format!("apr_{}", hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub struct RuntimeBuilder {
    tools: ToolRegistry,
    llms: LlmRegistry,
    approver: Option<Arc<dyn Approver>>,
    human: Option<Arc<dyn HumanInteractor>>,
    tracer: Option<Tracer>,
    trace_schema_writer: &'static str,
    default_model: String,
    model_catalog: ModelCatalog,
    model_catalog_root: Option<PathBuf>,
    rollout_seed: Option<u64>,
    replay_trace: Option<PathBuf>,
    replay_model_swap: Option<String>,
    replay_mutation: Option<(usize, serde_json::Value)>,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self {
            tools: ToolRegistry::default(),
            llms: LlmRegistry::default(),
            approver: None,
            human: None,
            tracer: None,
            trace_schema_writer: WRITER_INTERPRETER,
            default_model: String::new(),
            model_catalog: ModelCatalog::default(),
            model_catalog_root: None,
            rollout_seed: None,
            replay_trace: None,
            replay_model_swap: None,
            replay_mutation: None,
        }
    }
}

impl RuntimeBuilder {
    pub fn tool<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<serde_json::Value, RuntimeError>> + Send + 'static,
    {
        self.tools.register(name, handler);
        self
    }

    pub fn llm(mut self, adapter: Arc<dyn LlmAdapter>) -> Self {
        self.llms.register(adapter);
        self
    }

    pub fn approver(mut self, approver: Arc<dyn Approver>) -> Self {
        self.approver = Some(approver);
        self
    }

    pub fn human_interactor(mut self, human: Arc<dyn HumanInteractor>) -> Self {
        self.human = Some(human);
        self
    }

    pub fn tracer(mut self, tracer: Tracer) -> Self {
        self.tracer = Some(tracer);
        self
    }

    pub fn trace_schema_writer(mut self, writer: &'static str) -> Self {
        self.trace_schema_writer = writer;
        self
    }

    /// Open a JSONL trace file under `dir` with a fresh run id.
    pub fn trace_to(self, dir: &Path) -> Self {
        let tracer = Tracer::open(dir, fresh_run_id());
        self.tracer(tracer)
    }

    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    pub fn model(mut self, model: RegisteredModel) -> Self {
        self.model_catalog.register(model);
        self
    }

    pub fn model_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.model_catalog = catalog;
        self
    }

    pub fn model_catalog_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.model_catalog_root = Some(root.into());
        self
    }

    pub fn rollout_seed(mut self, seed: u64) -> Self {
        self.rollout_seed = Some(seed);
        self
    }

    pub fn replay_from(mut self, path: impl Into<PathBuf>) -> Self {
        self.replay_trace = Some(path.into());
        self
    }

    pub fn replay_model_swap(mut self, model: impl Into<String>) -> Self {
        self.replay_model_swap = Some(model.into());
        self
    }

    pub fn differential_replay_from(
        mut self,
        path: impl Into<PathBuf>,
        model: impl Into<String>,
    ) -> Self {
        self.replay_trace = Some(path.into());
        self.replay_model_swap = Some(model.into());
        self
    }

    pub fn mutation_replay_from(
        mut self,
        path: impl Into<PathBuf>,
        step_1based: usize,
        replacement: serde_json::Value,
    ) -> Self {
        self.replay_trace = Some(path.into());
        self.replay_mutation = Some((step_1based, replacement));
        self
    }

    pub fn build(self) -> Runtime {
        let mut model_catalog = self.model_catalog;
        let model_catalog_error = if model_catalog.is_empty() {
            let start = self
                .model_catalog_root
                .or_else(|| std::env::current_dir().ok());
            match start {
                Some(start) => match ModelCatalog::load_walking(&start) {
                    Ok(Some(loaded)) => {
                        model_catalog.extend(loaded);
                        None
                    }
                    Ok(None) => None,
                    Err(err) => Some(err),
                },
                None => None,
            }
        } else {
            None
        };
        let tracer = self.tracer.unwrap_or_else(Tracer::null);
        let recorder = Recorder::for_tracer(&tracer, self.trace_schema_writer).map(Arc::new);
        let (mode, replay_error, rollout_seed) = if let Some(path) = self.replay_trace {
            let replay_load = if let Some((step_1based, replacement)) = self.replay_mutation {
                ReplaySource::from_path_for_writer_with_mutation(
                    path,
                    self.trace_schema_writer,
                    step_1based,
                    replacement,
                )
            } else if let Some(model) = self.replay_model_swap {
                ReplaySource::from_path_for_writer_with_model(path, self.trace_schema_writer, model)
            } else {
                ReplaySource::from_path_for_writer(path, self.trace_schema_writer)
            };
            match replay_load {
                Ok(source) => (
                    RuntimeMode::Replay(source.clone()),
                    None,
                    source.initial_rollout_seed(),
                ),
                Err(err) => (
                    RuntimeMode::Live,
                    Some(err),
                    self.rollout_seed.unwrap_or_else(crate::tracing::now_ms),
                ),
            }
        } else {
            (
                RuntimeMode::Live,
                None,
                self.rollout_seed.unwrap_or_else(crate::tracing::now_ms),
            )
        };
        if let Some(recorder) = &recorder {
            recorder.emit_schema_header();
            recorder.emit_seed_read("rollout_default_seed", rollout_seed);
        }
        Runtime {
            tools: self.tools,
            llms: self.llms,
            approver: self
                .approver
                .unwrap_or_else(|| Arc::new(StdinApprover::new())),
            human: self
                .human
                .unwrap_or_else(|| Arc::new(StdinHumanInteractor::new())),
            tracer,
            recorder,
            mode,
            replay_error,
            default_model: self.default_model,
            model_catalog,
            model_catalog_error,
            rollout_state: Arc::new(AtomicU64::new(rollout_seed)),
            calibration: CalibrationStore::default(),
            prompt_cache: PromptCache::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::{ApprovalTokenScope, ProgrammaticApprover};
    use crate::llm::mock::MockAdapter;
    use serde_json::json;

    fn rt() -> Runtime {
        Runtime::builder()
            .tool("double", |args| async move {
                let n = args[0].as_i64().unwrap();
                Ok(json!(n * 2))
            })
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(
                MockAdapter::new("mock-1").reply("greet", json!("hi")),
            ))
            .default_model("mock-1")
            .build()
    }

    #[tokio::test]
    async fn call_tool_routes_through_registry() {
        let r = rt();
        let v = r.call_tool("double", vec![json!(5)]).await.unwrap();
        assert_eq!(v, json!(10));
    }

    #[tokio::test]
    async fn approval_gate_passes_when_approver_says_yes() {
        let r = rt();
        r.approval_gate("Anything", vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn approval_gate_emits_scoped_token_for_approved_request() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("approval.jsonl");
        let r = Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .tracer(Tracer::open_path(&trace_path, "approval-run"))
            .build();

        r.approval_gate("IssueRefund", vec![json!("ord_1"), json!(12.5)])
            .await
            .unwrap();

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        let token = events
            .iter()
            .find_map(|event| match event {
                TraceEvent::ApprovalTokenIssued {
                    token_id,
                    label,
                    args,
                    scope,
                    issued_at_ms,
                    expires_at_ms,
                    ..
                } => Some((
                    token_id,
                    label,
                    args,
                    scope,
                    *issued_at_ms,
                    *expires_at_ms,
                )),
                _ => None,
            })
            .expect("approval token event");

        assert!(token.0.starts_with("apr_"));
        assert_eq!(token.0.len(), 68);
        assert_eq!(token.1, "IssueRefund");
        assert_eq!(token.2, &vec![json!("ord_1"), json!(12.5)]);
        assert_eq!(token.3, "one_time");
        assert_eq!(token.5 - token.4, APPROVAL_TOKEN_TTL_MS);
    }

    #[test]
    fn approval_scope_violation_is_trace_visible() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("scope.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "scope-run"))
            .build();
        let mut token = ApprovalToken {
            token_id: "apr_limit".into(),
            label: "ChargeCard".into(),
            args: vec![json!(100.0)],
            scope: ApprovalTokenScope::AmountLimited { max_amount: 100.0 },
            issued_at_ms: 0,
            expires_at_ms: u64::MAX,
            uses_remaining: 1,
        };

        let err = r
            .validate_approval_token_scope(&mut token, "ChargeCard", &[json!(125.0)], None)
            .unwrap_err();
        assert!(matches!(err, RuntimeError::ApprovalFailed(_)));

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::ApprovalScopeViolation {
                token_id,
                label,
                reason,
                ..
            } if token_id == "apr_limit"
                && label == "ChargeCard"
                && reason.contains("exceeds token limit")
        )));
    }

    #[tokio::test]
    async fn approval_gate_blocks_when_approver_says_no() {
        let r = Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_no()))
            .build();
        let err = r.approval_gate("IssueRefund", vec![]).await.unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::ApprovalDenied { ref action } if action == "IssueRefund"
        ));
    }

    #[tokio::test]
    async fn call_llm_uses_default_model_when_request_blank() {
        let r = rt();
        let resp = r
            .call_llm(LlmRequest {
                prompt: "greet".into(),
                model: String::new(),
                rendered: "say hi".into(),
                args: vec![],
                output_schema: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.value, json!("hi"));
    }

    #[test]
    fn explicit_runtime_catalog_enables_capability_selection() {
        let runtime = Runtime::builder()
            .model(
                RegisteredModel::new("cheap-basic")
                    .capability("basic")
                    .cost_per_token_in(0.000001)
                    .cost_per_token_out(0.000001),
            )
            .model(
                RegisteredModel::new("cheap-expert")
                    .capability("expert")
                    .cost_per_token_in(0.000002)
                    .cost_per_token_out(0.000001),
            )
            .build();

        let selected = runtime
            .select_cheapest_model_for_capability("expert", 100, 50)
            .unwrap();
        assert_eq!(selected.model, "cheap-expert");
        assert_eq!(selected.capability_picked.as_deref(), Some("expert"));
    }

    #[test]
    fn builder_can_load_model_catalog_from_project_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("corvid.toml"),
            r#"
[llm.models.haiku]
capability = "basic"
cost_per_token_in = 0.00000025
cost_per_token_out = 0.00000125

[llm.models.opus]
capability = "expert"
cost_per_token_in = 0.000015
"#,
        )
        .unwrap();

        let runtime = Runtime::builder().model_catalog_root(dir.path()).build();
        let selected = runtime
            .select_cheapest_model_for_capability("expert", 10, 10)
            .unwrap();
        assert_eq!(selected.model, "opus");
    }

    #[test]
    fn describe_named_model_uses_runtime_catalog_when_available() {
        let runtime = Runtime::builder()
            .model(
                RegisteredModel::new("fast")
                    .capability("basic")
                    .cost_per_token_in(0.25)
                    .cost_per_token_out(0.5),
            )
            .build();

        let selected = runtime.describe_named_model("fast", 2, 3).unwrap();
        assert_eq!(selected.model, "fast");
        assert_eq!(selected.capability_picked.as_deref(), Some("basic"));
        assert!((selected.cost_estimate - 2.0).abs() < 1e-12);
    }

    #[test]
    fn rollout_extremes_choose_expected_variant() {
        let runtime = Runtime::builder().rollout_seed(7).build();
        assert!(!runtime.choose_rollout_variant(0.0).unwrap());
        assert!(runtime.choose_rollout_variant(100.0).unwrap());
    }

    #[test]
    fn rollout_seed_produces_stable_sequence_across_restarts() {
        let runtime_a = Runtime::builder().rollout_seed(12345).build();
        let runtime_b = Runtime::builder().rollout_seed(12345).build();

        let sequence_a: Vec<bool> = (0..8)
            .map(|_| runtime_a.choose_rollout_variant(37.5).unwrap())
            .collect();
        let sequence_b: Vec<bool> = (0..8)
            .map(|_| runtime_b.choose_rollout_variant(37.5).unwrap())
            .collect();

        assert_eq!(sequence_a, sequence_b);
    }

    #[test]
    fn builder_defaults_to_interpreter_trace_writer() {
        let runtime = Runtime::builder().build();
        assert_eq!(runtime.tracer().run_id(), "null");
        assert!(runtime.recorder().is_none());
        let builder = RuntimeBuilder::default();
        assert_eq!(builder.trace_schema_writer, WRITER_INTERPRETER);
    }

    #[test]
    fn differential_replay_builder_exposes_swap_model() {
        let builder = Runtime::builder()
            .differential_replay_from("trace.jsonl", "mock-2")
            .replay_model_swap("mock-3");
        assert_eq!(builder.replay_trace, Some(PathBuf::from("trace.jsonl")));
        assert_eq!(builder.replay_model_swap.as_deref(), Some("mock-3"));
    }
}
