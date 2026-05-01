//! LLM and tool dispatch methods on `Runtime`, plus the
//! approval-gate state machine and the human-in-the-loop ask /
//! choose helpers. These are the central call paths the
//! interpreter takes for every LLM, tool, approval, and human
//! interaction — they own the trace-event bracketing and the
//! replay-source consultation that those paths require.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::approvals::{ApprovalDecision, ApprovalRequest, ApprovalToken};
use crate::errors::RuntimeError;
use crate::human::{HumanChoiceRequest, HumanInputRequest};
use crate::llm::{LlmRequest, LlmRequestRef, LlmResponse};
use crate::prompt_cache::PromptCache;
use crate::tracing::now_ms;
use crate::usage::{normalized_total_tokens, LlmUsageRecord};
use corvid_trace_schema::{TraceEvent, WRITER_INTERPRETER};

use super::{Runtime, APPROVAL_TOKEN_SCOPE_ONE_TIME, APPROVAL_TOKEN_TTL_MS};

impl Runtime {
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
        let mut actual_model = live_model_override
            .as_deref()
            .unwrap_or(req.model)
            .to_string();
        let mut actual_adapter = if replay.is_some() {
            self.llms.adapter_name_for_model(&actual_model)
        } else {
            None
        };
        let mut result_trace_model = trace_model.to_string();
        let mut result_trace_model_version = trace_model_version.clone();
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
            match self.llms.call_with_adapter_name(&req).await {
                Ok(outcome) => {
                    actual_adapter = Some(outcome.adapter);
                    outcome.response
                }
                Err(primary_err) => {
                    let primary_error = primary_err.to_string();
                    self.emit_host_event(
                        "llm.provider_degraded",
                        serde_json::json!({
                            "prompt": req.prompt,
                            "model": req.model,
                            "provider": self.model_catalog.get(req.model).and_then(|model| model.provider.clone()),
                            "error": primary_error,
                        }),
                    );
                    let mut last_err = primary_err;
                    let fallbacks = self.model_catalog.compatible_fallbacks_for(
                        req.model,
                        estimate_tokens(trace_rendered),
                        0,
                    );
                    let mut fallback_response = None;
                    for fallback in fallbacks {
                        let fallback_req = req.with_model(&fallback.model);
                        match self.llms.call_with_adapter_name(&fallback_req).await {
                            Ok(outcome) => {
                                self.emit_host_event(
                                    "llm.provider_failover",
                                    serde_json::json!({
                                        "prompt": req.prompt,
                                        "from_model": req.model,
                                        "from_provider": self.model_catalog.get(req.model).and_then(|model| model.provider.clone()),
                                        "to_model": fallback.model.clone(),
                                        "to_provider": fallback.provider.clone(),
                                        "adapter": outcome.adapter,
                                    }),
                                );
                                actual_model = fallback.model;
                                actual_adapter = Some(outcome.adapter);
                                result_trace_model = actual_model.clone();
                                result_trace_model_version = self.model_version(&actual_model);
                                fallback_response = Some(outcome.response);
                                break;
                            }
                            Err(err) => {
                                self.emit_host_event(
                                    "llm.provider_degraded",
                                    serde_json::json!({
                                        "prompt": req.prompt,
                                        "model": fallback.model.clone(),
                                        "provider": fallback.provider.clone(),
                                        "error": err.to_string(),
                                    }),
                                );
                                last_err = err;
                            }
                        }
                    }
                    fallback_response.ok_or(last_err)?
                }
            }
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
        let cost_usd = if actual_model.is_empty() {
            0.0
        } else {
            self.model_catalog
                .describe_named_model(
                    &actual_model,
                    resp.usage.prompt_tokens as u64,
                    resp.usage.completion_tokens as u64,
                )
                .cost_estimate
        };
        let model_metadata = self.model_catalog.get(&actual_model);
        let provider = model_metadata.and_then(|model| model.provider.clone());
        let privacy_tier = model_metadata.and_then(|model| model.privacy_tier.clone());
        let total_tokens = normalized_total_tokens(resp.usage);
        let usage_record = LlmUsageRecord {
            ts_ms: now_ms(),
            prompt: req.prompt.to_string(),
            model: actual_model.clone(),
            provider: provider.clone(),
            adapter: actual_adapter.clone(),
            privacy_tier: privacy_tier.clone(),
            prompt_tokens: resp.usage.prompt_tokens as u64,
            completion_tokens: resp.usage.completion_tokens as u64,
            total_tokens,
            cost_usd,
            local: provider.as_deref() == Some("ollama") || privacy_tier.as_deref() == Some("local"),
        };
        self.usage_ledger.record(usage_record.clone());
        self.emit_host_event(
            "llm.usage",
            serde_json::json!({
                "prompt": usage_record.prompt,
                "model": usage_record.model,
                "provider": usage_record.provider,
                "adapter": usage_record.adapter,
                "privacy_tier": usage_record.privacy_tier,
                "prompt_tokens": usage_record.prompt_tokens,
                "completion_tokens": usage_record.completion_tokens,
                "total_tokens": usage_record.total_tokens,
                "cost_usd": usage_record.cost_usd,
                "currency": "USD",
                "unit": "token",
                "local": usage_record.local,
            }),
        );
        crate::observation_handles::record_llm_usage(resp.usage, cost_usd);
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::LlmResult {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                prompt: req.prompt.to_string(),
                model: if result_trace_model.is_empty() {
                    None
                } else {
                    Some(result_trace_model)
                },
                model_version: result_trace_model_version,
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

fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4).max(1)
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
