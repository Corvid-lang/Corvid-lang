use super::{ExprFlow, Interpreter};
use crate::conv::json_to_value;
use crate::errors::{InterpError, InterpErrorKind};
use crate::step::{StepAction, StepEvent};
use crate::value::{value_confidence, ResumeTokenValue, StreamChunk, StreamResumeContext, Value};
use crate::value_to_json;
use async_recursion::async_recursion;
use corvid_ast::Span;
use corvid_ir::{IrEnsembleWeighting, IrPrompt, IrRoutePattern};
use corvid_resolve::DefId;
use corvid_runtime::{
    contradiction_flag, majority_vote, trace_text, weighted_vote, LlmRequest, TokenUsage,
    TraceEvent,
};
use corvid_types::Type;
use tokio::task::JoinSet;

const DEFAULT_COMPLETION_TOKEN_ESTIMATE: u64 = 256;

struct PromptCallResult {
    value: Value,
    cost: f64,
    confidence: f64,
    tokens: u64,
    cost_charged: bool,
}

impl<'ir> Interpreter<'ir> {
    fn emit_model_selected(
        &self,
        callee_name: &str,
        model: String,
        model_version: Option<String>,
        capability_required: Option<String>,
        capability_picked: Option<String>,
        output_format_required: Option<String>,
        output_format_picked: Option<String>,
        cost_estimate: f64,
        arm_index: Option<usize>,
        stage_index: Option<usize>,
    ) {
        self.runtime.tracer().emit(TraceEvent::ModelSelected {
            ts_ms: corvid_runtime::now_ms(),
            run_id: self.runtime.tracer().run_id().to_string(),
            prompt: callee_name.to_string(),
            model,
            model_version,
            capability_required,
            capability_picked,
            output_format_required,
            output_format_picked,
            cost_estimate,
            arm_index,
            stage_index,
        });
    }

    fn select_named_prompt_model(
        &self,
        callee_name: &str,
        model_name: &str,
        required_output_format: Option<&str>,
        prompt_tokens: u64,
        completion_tokens: u64,
        arm_index: Option<usize>,
        stage_index: Option<usize>,
        span: Span,
    ) -> Result<String, InterpError> {
        let selection = self
            .runtime
            .describe_named_model(model_name, prompt_tokens, completion_tokens)
            .map_err(|err| InterpError::new(InterpErrorKind::Runtime(err), span))?;
        if let Some(required) = required_output_format {
            if selection.output_format_picked.as_deref() != Some(required) {
                return Err(InterpError::new(
                    InterpErrorKind::Runtime(corvid_runtime::RuntimeError::ModelOutputFormatMismatch {
                        prompt: callee_name.to_string(),
                        model: selection.model.clone(),
                        required_output_format: required.to_string(),
                        model_output_format: selection.output_format_picked.clone(),
                    }),
                    span,
                ));
            }
        }
        self.emit_model_selected(
            callee_name,
            selection.model.clone(),
            selection.version,
            selection.capability_required,
            selection.capability_picked,
            required_output_format.map(ToString::to_string),
            selection.output_format_picked,
            selection.cost_estimate,
            arm_index,
            stage_index,
        );
        Ok(selection.model)
    }

    async fn select_prompt_model(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        rendered: &str,
        arg_values: &[Value],
        span: Span,
    ) -> Result<Option<String>, InterpError> {
        let prompt_tokens = super::effect_compose::estimate_tokens(rendered);
        let completion_tokens = prompt
            .max_tokens
            .unwrap_or(DEFAULT_COMPLETION_TOKEN_ESTIMATE);

        if !prompt.route.is_empty() {
            let saved_env = self.env.clone();
            let saved_names = self.local_names.clone();
            for (param, value) in prompt.params.iter().zip(arg_values.iter()) {
                self.env.bind(param.local_id, value.clone());
                self.local_names.insert(param.local_id, param.name.clone());
            }
            let outcome = self
                .select_prompt_route_model(
                    prompt,
                    callee_name,
                    prompt_tokens,
                    completion_tokens,
                    span,
                )
                .await;
            self.env = saved_env;
            self.local_names = saved_names;
            return outcome;
        }

        let required_capability = prompt.capability_required.as_deref();
        let required_output_format = prompt.output_format_required.as_deref();
        if required_capability.is_none() && required_output_format.is_none() {
            return Ok(None);
        }
        let selection = self
            .runtime
            .select_cheapest_model_for_requirements(
                required_capability,
                required_output_format,
                prompt_tokens,
                completion_tokens,
            )
            .map_err(|err| InterpError::new(InterpErrorKind::Runtime(err), span))?;
        self.emit_model_selected(
            callee_name,
            selection.model.clone(),
            selection.version,
            selection.capability_required,
            selection.capability_picked,
            selection.output_format_required,
            selection.output_format_picked,
            selection.cost_estimate,
            None,
            None,
        );
        Ok(Some(selection.model))
    }

    async fn select_prompt_route_model(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
        span: Span,
    ) -> Result<Option<String>, InterpError> {
        for (arm_index, arm) in prompt.route.iter().enumerate() {
            let matched = match &arm.pattern {
                IrRoutePattern::Wildcard => true,
                IrRoutePattern::Guard(expr) => {
                    let guard_value = match self.eval_expr(expr).await?.into_value() {
                        Ok(v) | Err(v) => v,
                    };
                    super::expr::require_bool(&guard_value, expr.span, "route guard")?
                }
            };
            if !matched {
                continue;
            }
            return self
                .select_named_prompt_model(
                    callee_name,
                    &arm.model_name,
                    prompt.output_format_required.as_deref(),
                    prompt_tokens,
                    completion_tokens,
                    Some(arm_index),
                    None,
                    span,
                )
                .map(Some);
        }

        Err(InterpError::new(
            InterpErrorKind::Runtime(corvid_runtime::RuntimeError::NoMatchingRoute {
                prompt: callee_name.to_string(),
            }),
            span,
        ))
    }

    fn prompt_call_cost(
        &self,
        prompt: &IrPrompt,
        model_name: &str,
        rendered: &str,
        usage: TokenUsage,
    ) -> f64 {
        let prompt_tokens = if usage.prompt_tokens > 0 {
            usage.prompt_tokens as u64
        } else {
            super::effect_compose::estimate_tokens(rendered)
        };
        let completion_tokens = if usage.completion_tokens > 0 {
            usage.completion_tokens as u64
        } else if usage.total_tokens > usage.prompt_tokens {
            (usage.total_tokens - usage.prompt_tokens) as u64
        } else if usage.total_tokens > 0 {
            usage.total_tokens as u64
        } else {
            0
        };
        match self
            .runtime
            .describe_named_model(model_name, prompt_tokens, completion_tokens)
        {
            Ok(selection) if selection.cost_estimate > 0.0 => selection.cost_estimate,
            _ => prompt.effect_cost,
        }
    }

    fn decode_prompt_response(
        &self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        rendered: &str,
        actual_model: &str,
        response_value: serde_json::Value,
        usage: TokenUsage,
        response_confidence: Option<f64>,
        calibration_actual: Option<bool>,
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        let declared_result_ty = match &prompt.return_ty {
            Type::Stream(inner) => inner.as_ref(),
            other => other,
        };
        let result_ty = match declared_result_ty {
            Type::Grounded(inner) => inner.as_ref(),
            other => other,
        };

        let value = json_to_value(response_value, result_ty, &self.types_by_id).map_err(|e| {
            InterpError::new(
                InterpErrorKind::Marshal(format!("prompt `{callee_name}`: {e}")),
                span,
            )
        })?;

        if let Some(param_idx) = prompt.cites_strictly_param {
            if let Some(ctx_value) = arg_values.get(param_idx) {
                let ctx_text = match ctx_value {
                    Value::Grounded(g) => value_to_json(&g.inner.get()).to_string(),
                    other => value_to_json(other).to_string(),
                };
                let response_text = value_to_json(&value).to_string();
                if !corvid_runtime::citation::citation_verified(&ctx_text, &response_text) {
                    return Err(InterpError::new(
                        InterpErrorKind::Other(format!(
                            "citation verification failed for prompt `{callee_name}`: \
                             response does not reference content from the cited context parameter"
                        )),
                        span,
                    ));
                }
            }
        }

        let mut merged_chain = crate::ProvenanceChain::new();
        let mut has_grounded_input = false;
        for arg in arg_values {
            if let Value::Grounded(g) = arg {
                merged_chain.merge(&g.provenance);
                has_grounded_input = true;
            }
        }
        let value = if has_grounded_input {
            merged_chain.add_prompt_transform(callee_name, corvid_runtime::now_ms());
            Value::Grounded(crate::value::GroundedValue::with_confidence(
                value,
                merged_chain,
                super::effect_compose::composed_confidence(arg_values),
            ))
        } else {
            value
        };
        let value = response_confidence
            .map(|confidence| {
                let combined = confidence.min(value_confidence(&value));
                super::effect_compose::with_value_confidence(value.clone(), combined)
            })
            .unwrap_or(value);

        let confidence = super::effect_compose::prompt_effective_confidence(prompt, &value);
        if prompt.calibrated {
            if let Some(actual_correct) = calibration_actual {
                self.runtime.record_calibration(
                    callee_name,
                    actual_model,
                    confidence,
                    actual_correct,
                );
            }
        }
        let tokens = if usage.completion_tokens > 0 {
            usage.completion_tokens as u64
        } else if usage.total_tokens > 0 {
            usage.total_tokens as u64
        } else {
            super::effect_compose::estimate_tokens(&value_to_json(&value).to_string())
        };
        let cost = self.prompt_call_cost(prompt, actual_model, rendered, usage);

        Ok(PromptCallResult {
            value,
            cost,
            confidence,
            tokens,
            cost_charged: false,
        })
    }

    async fn execute_prompt_call(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        rendered: &str,
        selected_model: Option<String>,
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        let json_args: Vec<serde_json::Value> = arg_values.iter().map(value_to_json).collect();

        if self.should_yield_boundary() {
            let action = self
                .maybe_yield(StepEvent::BeforePromptCall {
                    prompt_name: callee_name.to_string(),
                    rendered: rendered.to_string(),
                    model: selected_model.clone(),
                    input_confidence: super::effect_compose::composed_confidence(arg_values),
                    span,
                    env: self.env_snapshot(),
                })
                .await?;
            if let StepAction::Override(val) = action {
                let result_ty = match &prompt.return_ty {
                    Type::Stream(inner) => inner.as_ref(),
                    other => other,
                };
                let value = json_to_value(val, result_ty, &self.types_by_id).map_err(|e| {
                    InterpError::new(
                        InterpErrorKind::Marshal(format!(
                            "prompt `{callee_name}` override: {e}"
                        )),
                        span,
                    )
                })?;
                let confidence = super::effect_compose::prompt_effective_confidence(prompt, &value);
                let tokens = super::effect_compose::estimate_tokens(&value_to_json(&value).to_string());
                let model_name = selected_model
                    .clone()
                    .unwrap_or_else(|| self.runtime.default_model().to_string());
                let cost = self.prompt_call_cost(
                    prompt,
                    &model_name,
                    rendered,
                    TokenUsage {
                        prompt_tokens: super::effect_compose::estimate_tokens(rendered) as u32,
                        completion_tokens: tokens as u32,
                        total_tokens: (super::effect_compose::estimate_tokens(rendered) + tokens)
                            as u32,
                    },
                );
                return Ok(PromptCallResult {
                    value,
                    cost,
                    confidence,
                    tokens,
                    cost_charged: false,
                });
            }
        }

        let result_ty = match &prompt.return_ty {
            Type::Stream(inner) => inner.as_ref(),
            other => other,
        };
        let output_schema = Some(crate::schema::schema_for(result_ty, &self.types_by_id));
        let req = LlmRequest {
            prompt: callee_name.to_string(),
            model: selected_model.clone().unwrap_or_default(),
            rendered: rendered.to_string(),
            args: json_args,
            output_schema,
        };
        let actual_model = if req.model.is_empty() {
            self.runtime.default_model().to_string()
        } else {
            req.model.clone()
        };
        let start = std::time::Instant::now();
        let resp = self
            .runtime
            .call_llm_cacheable(req, prompt.cacheable)
            .await
            .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        if self.should_yield_boundary() {
            let action = self
                .maybe_yield(StepEvent::AfterPromptCall {
                    prompt_name: callee_name.to_string(),
                    result: resp.value.clone(),
                    result_confidence: prompt
                        .effect_confidence
                        .min(super::effect_compose::composed_confidence(arg_values)),
                    elapsed_ms,
                    span,
                })
                .await?;
            if let StepAction::Override(val) = action {
                let value = json_to_value(val, result_ty, &self.types_by_id).map_err(|e| {
                    InterpError::new(
                        InterpErrorKind::Marshal(format!(
                            "prompt `{callee_name}` override: {e}"
                        )),
                        span,
                    )
                })?;
                let confidence = super::effect_compose::prompt_effective_confidence(prompt, &value);
                let tokens = super::effect_compose::estimate_tokens(&value_to_json(&value).to_string());
                let cost = self.prompt_call_cost(
                    prompt,
                    &actual_model,
                    rendered,
                    TokenUsage {
                        prompt_tokens: super::effect_compose::estimate_tokens(rendered) as u32,
                        completion_tokens: tokens as u32,
                        total_tokens: (super::effect_compose::estimate_tokens(rendered) + tokens)
                            as u32,
                    },
                );
                return Ok(PromptCallResult {
                    value,
                    cost,
                    confidence,
                    tokens,
                    cost_charged: false,
                });
            }
        }

        self.decode_prompt_response(
            prompt,
            callee_name,
            arg_values,
            rendered,
            &actual_model,
            resp.value,
            resp.usage,
            resp.confidence,
            resp.calibration.map(|c| c.actual_correct),
            span,
        )
    }

    async fn finalize_prompt_result(
        &self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        result: PromptCallResult,
        span: Span,
    ) -> Result<ExprFlow, InterpError> {
        if matches!(&prompt.return_ty, Type::Stream(_)) {
            let chunk = StreamChunk::with_metrics(
                result.value,
                result.cost,
                result.confidence,
                result.tokens,
            );
            if let Some(limit) = prompt.max_tokens {
                if chunk.tokens > limit {
                    return self
                        .singleton_stream_error(
                            InterpError::new(
                                InterpErrorKind::TokenLimitExceeded {
                                    limit,
                                    used: chunk.tokens,
                                },
                                span,
                            ),
                            super::effect_compose::prompt_backpressure(prompt),
                        )
                        .await
                        .map(ExprFlow::Value);
                }
            }
            if let Some(floor) = prompt.min_confidence {
                if chunk.confidence < floor {
                    return self
                        .singleton_stream_error(
                            InterpError::new(
                                InterpErrorKind::ConfidenceFloorBreached {
                                    floor,
                                    actual: chunk.confidence,
                                },
                                span,
                            ),
                            super::effect_compose::prompt_backpressure(prompt),
                        )
                        .await
                        .map(ExprFlow::Value);
                }
            }
            let value = self
                .singleton_stream(chunk, super::effect_compose::prompt_backpressure(prompt))
                .await?;
            if let Value::Stream(stream) = &value {
                stream.set_resume_context(StreamResumeContext {
                    prompt_name: callee_name.to_string(),
                    args: arg_values.to_vec(),
                    provider_session: None,
                });
            }
            Ok(ExprFlow::Value(value))
        } else {
            Ok(ExprFlow::Value(
                super::effect_compose::with_value_confidence(result.value, result.confidence),
            ))
        }
    }

    async fn maybe_escalate_stream_result(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        result: PromptCallResult,
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        if !matches!(&prompt.return_ty, Type::Stream(_)) {
            return Ok(result);
        }
        let Some(threshold) = prompt.min_confidence else {
            return Ok(result);
        };
        if result.confidence >= threshold {
            return Ok(result);
        }
        let Some(escalate_to) = prompt.escalate_to.as_deref() else {
            return Ok(result);
        };

        let rendered = render_prompt(prompt, arg_values);
        let partial = value_to_json(&result.value);
        let continuation_rendered = format!(
            "{rendered}\n\nContinue from partial output:\n{}",
            trace_text(&partial)
        );
        let prompt_tokens = super::effect_compose::estimate_tokens(&continuation_rendered);
        let completion_tokens = prompt
            .max_tokens
            .unwrap_or(DEFAULT_COMPLETION_TOKEN_ESTIMATE);
        let selected_model = self.select_named_prompt_model(
            callee_name,
            escalate_to,
            prompt.output_format_required.as_deref(),
            prompt_tokens,
            completion_tokens,
            None,
            None,
            span,
        )?;
        self.runtime.tracer().emit(TraceEvent::StreamUpgrade {
            ts_ms: corvid_runtime::now_ms(),
            run_id: self.runtime.tracer().run_id().to_string(),
            prompt: callee_name.to_string(),
            to_model: selected_model.clone(),
            confidence_observed: result.confidence,
            threshold,
            partial: partial.clone(),
        });
        let mut upgraded = self
            .execute_prompt_call(
                prompt,
                callee_name,
                arg_values,
                &continuation_rendered,
                Some(selected_model),
                span,
            )
            .await?;
        upgraded.cost += result.cost;
        upgraded.tokens += result.tokens;
        Ok(upgraded)
    }

    fn prompt_by_id(
        &self,
        def_id: DefId,
        prompt_name: &str,
        span: Span,
    ) -> Result<&'ir IrPrompt, InterpError> {
        self.prompts_by_id.get(&def_id).copied().ok_or_else(|| {
            InterpError::new(
                InterpErrorKind::DispatchFailed(format!(
                    "prompt `{prompt_name}` is missing from the IR"
                )),
                span,
            )
        })
    }

    #[async_recursion]
    async fn dispatch_prompt(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        let rendered = render_prompt(prompt, arg_values);
        if let Some(spec) = &prompt.ensemble {
            if self.should_yield_boundary() {
                let action = self
                    .maybe_yield(StepEvent::BeforePromptCall {
                        prompt_name: callee_name.to_string(),
                        rendered: rendered.clone(),
                        model: None,
                        input_confidence: super::effect_compose::composed_confidence(arg_values),
                        span,
                        env: self.env_snapshot(),
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(
                        |e| {
                            InterpError::new(
                                InterpErrorKind::Marshal(format!(
                                    "prompt `{callee_name}` override: {e}"
                                )),
                                span,
                            )
                        },
                    )?;
                    return Ok(PromptCallResult {
                        confidence: super::effect_compose::prompt_effective_confidence(prompt, &value),
                        tokens: super::effect_compose::estimate_tokens(
                            &value_to_json(&value).to_string(),
                        ),
                        cost: prompt.effect_cost,
                        value,
                        cost_charged: false,
                    });
                }
            }

            let prompt_tokens = super::effect_compose::estimate_tokens(&rendered);
            let completion_tokens = prompt
                .max_tokens
                .unwrap_or(DEFAULT_COMPLETION_TOKEN_ESTIMATE);
            let result_ty = match &prompt.return_ty {
                Type::Stream(inner) => inner.as_ref(),
                other => other,
            };
            let output_schema = Some(crate::schema::schema_for(result_ty, &self.types_by_id));
            let json_args: Vec<serde_json::Value> = arg_values.iter().map(value_to_json).collect();

            let mut requests = Vec::with_capacity(spec.models.len());
            for member in &spec.models {
                let selected_model = self.select_named_prompt_model(
                    callee_name,
                    &member.name,
                    prompt.output_format_required.as_deref(),
                    prompt_tokens,
                    completion_tokens,
                    None,
                    None,
                    span,
                )?;
                requests.push((
                    selected_model.clone(),
                    LlmRequest {
                        prompt: callee_name.to_string(),
                        model: selected_model,
                        rendered: rendered.clone(),
                        args: json_args.clone(),
                        output_schema: output_schema.clone(),
                    },
                ));
            }

            let ensemble_start = std::time::Instant::now();
            let mut join_set = JoinSet::new();
            for (index, (model_name, req)) in requests.into_iter().enumerate() {
                let runtime = self.runtime.clone();
                let cacheable = prompt.cacheable;
                join_set.spawn(async move {
                    let response = runtime.call_llm_cacheable(req, cacheable).await;
                    (index, model_name, response)
                });
            }

            let mut member_results: Vec<Option<(String, PromptCallResult)>> =
                (0..spec.models.len()).map(|_| None).collect();
            while let Some(joined) = join_set.join_next().await {
                let (index, model_name, response) = joined.map_err(|err| {
                    InterpError::new(
                        InterpErrorKind::Other(format!(
                            "ensemble task for prompt `{callee_name}` failed: {err}"
                        )),
                        span,
                    )
                })?;
                let response =
                    response.map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
                let result = self.decode_prompt_response(
                    prompt,
                    callee_name,
                    arg_values,
                    &rendered,
                    &model_name,
                    response.value,
                    response.usage,
                    response.confidence,
                    response.calibration.map(|c| c.actual_correct),
                    span,
                )?;
                member_results[index] = Some((model_name, result));
            }

            let member_results: Vec<(String, PromptCallResult)> = member_results
                .into_iter()
                .map(|entry| entry.expect("ensemble member result missing"))
                .collect();
            let members: Vec<String> =
                member_results.iter().map(|(model, _)| model.clone()).collect();
            let results: Vec<String> = member_results
                .iter()
                .map(|(_, result)| super::effect_compose::vote_text(&result.value))
                .collect();
            let weights = match spec.weighting {
                Some(IrEnsembleWeighting::AccuracyHistory) => Some(
                    members
                        .iter()
                        .map(|model| {
                            self.runtime
                                .calibration_stats(callee_name, model)
                                .map(|stats| stats.accuracy)
                                .unwrap_or(1.0)
                        })
                        .collect::<Vec<_>>(),
                ),
                None => None,
            };
            let vote = if let Some(weights) = &weights {
                weighted_vote(&results, weights)
            } else {
                majority_vote(&results)
            };
            let mut total_cost: f64 = member_results.iter().map(|(_, result)| result.cost).sum();
            let mut total_tokens: u64 = member_results.iter().map(|(_, result)| result.tokens).sum();
            let min_confidence = member_results
                .iter()
                .map(|(_, result)| result.confidence)
                .fold(1.0_f64, f64::min);
            let combined_confidence = min_confidence * vote.agreement_rate;
            let disagreed = vote.agreement_rate < 1.0 - f64::EPSILON;
            let mut escalated_to = None;
            let (winner_value, final_confidence) =
                if disagreed {
                    if let Some(escalation) = &spec.disagreement_escalation {
                        let selected_model = self.select_named_prompt_model(
                            callee_name,
                            &escalation.name,
                            prompt.output_format_required.as_deref(),
                            prompt_tokens,
                            completion_tokens,
                            None,
                            None,
                            span,
                        )?;
                        let response = self
                            .runtime
                            .call_llm_cacheable(
                                LlmRequest {
                                    prompt: callee_name.to_string(),
                                    model: selected_model.clone(),
                                    rendered: rendered.clone(),
                                    args: json_args.clone(),
                                    output_schema: output_schema.clone(),
                                },
                                prompt.cacheable,
                            )
                            .await
                            .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
                        let result = self.decode_prompt_response(
                            prompt,
                            callee_name,
                            arg_values,
                            &rendered,
                            &selected_model,
                            response.value,
                            response.usage,
                            response.confidence,
                            response.calibration.map(|c| c.actual_correct),
                            span,
                        )?;
                        total_cost += result.cost;
                        total_tokens += result.tokens;
                        escalated_to = Some(selected_model);
                        (result.value, result.confidence)
                    } else {
                        let winner_index = results
                            .iter()
                            .position(|result| result == &vote.winner)
                            .expect("winner must be one of the results");
                        (
                            super::effect_compose::with_value_confidence(
                                member_results[winner_index].1.value.clone(),
                                combined_confidence,
                            ),
                            combined_confidence,
                        )
                    }
                } else {
                    let winner_index = results
                        .iter()
                        .position(|result| result == &vote.winner)
                        .expect("winner must be one of the results");
                    (
                        super::effect_compose::with_value_confidence(
                            member_results[winner_index].1.value.clone(),
                            combined_confidence,
                        ),
                        combined_confidence,
                    )
                };

            self.runtime.tracer().emit(TraceEvent::EnsembleVote {
                ts_ms: corvid_runtime::now_ms(),
                run_id: self.runtime.tracer().run_id().to_string(),
                prompt: callee_name.to_string(),
                members,
                results: results.clone(),
                winner: vote.winner.clone(),
                agreement_rate: vote.agreement_rate,
                strategy: match spec.weighting {
                    Some(IrEnsembleWeighting::AccuracyHistory) => {
                        "majority weighted_by accuracy_history".to_string()
                    }
                    None => "majority".to_string(),
                },
                weights,
                escalated_to: escalated_to.clone(),
            });

            if self.should_yield_boundary() {
                let action = self
                    .maybe_yield(StepEvent::AfterPromptCall {
                        prompt_name: callee_name.to_string(),
                        result: value_to_json(&winner_value),
                        result_confidence: final_confidence,
                        elapsed_ms: ensemble_start.elapsed().as_millis() as u64,
                        span,
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(
                        |e| {
                            InterpError::new(
                                InterpErrorKind::Marshal(format!(
                                    "prompt `{callee_name}` override: {e}"
                                )),
                                span,
                            )
                        },
                    )?;
                    return Ok(PromptCallResult {
                        confidence: super::effect_compose::prompt_effective_confidence(prompt, &value),
                        tokens: super::effect_compose::estimate_tokens(
                            &value_to_json(&value).to_string(),
                        ),
                        cost: total_cost,
                        value,
                        cost_charged: false,
                    });
                }
            }

            Ok(PromptCallResult {
                value: winner_value,
                cost: total_cost,
                confidence: final_confidence,
                tokens: total_tokens,
                cost_charged: false,
            })
        } else if let Some(spec) = &prompt.adversarial {
            if self.should_yield_boundary() {
                let action = self
                    .maybe_yield(StepEvent::BeforePromptCall {
                        prompt_name: callee_name.to_string(),
                        rendered: rendered.clone(),
                        model: None,
                        input_confidence: super::effect_compose::composed_confidence(arg_values),
                        span,
                        env: self.env_snapshot(),
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(
                        |e| {
                            InterpError::new(
                                InterpErrorKind::Marshal(format!(
                                    "prompt `{callee_name}` override: {e}"
                                )),
                                span,
                            )
                        },
                    )?;
                    return Ok(PromptCallResult {
                        confidence: super::effect_compose::prompt_effective_confidence(prompt, &value),
                        tokens: super::effect_compose::estimate_tokens(
                            &value_to_json(&value).to_string(),
                        ),
                        cost: prompt.effect_cost,
                        value,
                        cost_charged: false,
                    });
                }
            }

            let pipeline_start = std::time::Instant::now();
            let proposer = self.prompt_by_id(spec.proposer_def_id, &spec.proposer_name, span)?;
            let proposed = self
                .dispatch_prompt(proposer, &spec.proposer_name, arg_values, span)
                .await?;
            if !proposed.cost_charged && !matches!(&proposer.return_ty, Type::Stream(_)) {
                self.charge_cost(proposed.cost, span)?;
            }

            let challenge_args = vec![proposed.value.clone()];
            let challenger = self.prompt_by_id(spec.challenger_def_id, &spec.challenger_name, span)?;
            let challenge = self
                .dispatch_prompt(challenger, &spec.challenger_name, &challenge_args, span)
                .await?;
            if !challenge.cost_charged && !matches!(&challenger.return_ty, Type::Stream(_)) {
                self.charge_cost(challenge.cost, span)?;
            }

            let adjudicator =
                self.prompt_by_id(spec.adjudicator_def_id, &spec.adjudicator_name, span)?;
            let verdict_args = vec![proposed.value.clone(), challenge.value.clone()];
            let verdict = self
                .dispatch_prompt(adjudicator, &spec.adjudicator_name, &verdict_args, span)
                .await?;
            if !verdict.cost_charged && !matches!(&adjudicator.return_ty, Type::Stream(_)) {
                self.charge_cost(verdict.cost, span)?;
            }

            let proposed_json = value_to_json(&proposed.value);
            let challenge_json = value_to_json(&challenge.value);
            let verdict_json = value_to_json(&verdict.value);
            let contradiction = contradiction_flag(callee_name, &verdict_json)
                .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
            if contradiction {
                self.runtime.tracer().emit(TraceEvent::AdversarialContradiction {
                    ts_ms: corvid_runtime::now_ms(),
                    run_id: self.runtime.tracer().run_id().to_string(),
                    prompt: callee_name.to_string(),
                    proposed: trace_text(&proposed_json),
                    challenge: trace_text(&challenge_json),
                    verdict: verdict_json.clone(),
                });
            }
            self.runtime
                .tracer()
                .emit(TraceEvent::AdversarialPipelineCompleted {
                    ts_ms: corvid_runtime::now_ms(),
                    run_id: self.runtime.tracer().run_id().to_string(),
                    prompt: callee_name.to_string(),
                    contradiction,
                });

            if self.should_yield_boundary() {
                let action = self
                    .maybe_yield(StepEvent::AfterPromptCall {
                        prompt_name: callee_name.to_string(),
                        result: verdict_json.clone(),
                        result_confidence: proposed
                            .confidence
                            .min(challenge.confidence)
                            .min(verdict.confidence),
                        elapsed_ms: pipeline_start.elapsed().as_millis() as u64,
                        span,
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(
                        |e| {
                            InterpError::new(
                                InterpErrorKind::Marshal(format!(
                                    "prompt `{callee_name}` override: {e}"
                                )),
                                span,
                            )
                        },
                    )?;
                    return Ok(PromptCallResult {
                        confidence: super::effect_compose::prompt_effective_confidence(prompt, &value),
                        tokens: super::effect_compose::estimate_tokens(
                            &value_to_json(&value).to_string(),
                        ),
                        cost: proposed.cost + challenge.cost + verdict.cost,
                        value,
                        cost_charged: true,
                    });
                }
            }

            Ok(PromptCallResult {
                value: verdict.value,
                cost: proposed.cost + challenge.cost + verdict.cost,
                confidence: proposed
                    .confidence
                    .min(challenge.confidence)
                    .min(verdict.confidence),
                tokens: proposed.tokens + challenge.tokens + verdict.tokens,
                cost_charged: true,
            })
        } else if let Some(spec) = &prompt.rollout {
            let prompt_tokens = super::effect_compose::estimate_tokens(&rendered);
            let completion_tokens = prompt
                .max_tokens
                .unwrap_or(DEFAULT_COMPLETION_TOKEN_ESTIMATE);
            let chosen_model = if self
                .runtime
                .choose_rollout_variant(spec.variant_percent)
                .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?
            {
                spec.variant_name.clone()
            } else {
                spec.baseline_name.clone()
            };
            self.runtime.tracer().emit(TraceEvent::AbVariantChosen {
                ts_ms: corvid_runtime::now_ms(),
                run_id: self.runtime.tracer().run_id().to_string(),
                prompt: callee_name.to_string(),
                variant: spec.variant_name.clone(),
                baseline: spec.baseline_name.clone(),
                rollout_pct: spec.variant_percent,
                chosen: chosen_model.clone(),
            });
            let selected_model = self.select_named_prompt_model(
                callee_name,
                &chosen_model,
                prompt.output_format_required.as_deref(),
                prompt_tokens,
                completion_tokens,
                None,
                None,
                span,
            )?;
            self.execute_prompt_call(
                prompt,
                callee_name,
                arg_values,
                &rendered,
                Some(selected_model),
                span,
            )
            .await
        } else if !prompt.progressive.is_empty() {
            let prompt_tokens = super::effect_compose::estimate_tokens(&rendered);
            let completion_tokens = prompt
                .max_tokens
                .unwrap_or(DEFAULT_COMPLETION_TOKEN_ESTIMATE);
            let stage_sequence: Vec<String> = prompt
                .progressive
                .iter()
                .map(|stage| stage.model_name.clone())
                .collect();
            for (stage_index, stage) in prompt.progressive.iter().enumerate() {
                let selected_model = self.select_named_prompt_model(
                    callee_name,
                    &stage.model_name,
                    prompt.output_format_required.as_deref(),
                    prompt_tokens,
                    completion_tokens,
                    None,
                    Some(stage_index),
                    span,
                )?;
                let result = self
                    .execute_prompt_call(
                        prompt,
                        callee_name,
                        arg_values,
                        &rendered,
                        Some(selected_model),
                        span,
                    )
                    .await?;
                if !matches!(&prompt.return_ty, Type::Stream(_)) {
                    self.charge_cost(result.cost, span)?;
                }
                let result = PromptCallResult {
                    cost_charged: !matches!(&prompt.return_ty, Type::Stream(_)),
                    ..result
                };
                match stage.threshold {
                    None => {
                        if stage_index > 0 {
                            self.runtime.tracer().emit(TraceEvent::ProgressiveExhausted {
                                ts_ms: corvid_runtime::now_ms(),
                                run_id: self.runtime.tracer().run_id().to_string(),
                                prompt: callee_name.to_string(),
                                stages: stage_sequence.clone(),
                            });
                        }
                        return Ok(result);
                    }
                    Some(threshold) if result.confidence >= threshold => {
                        return Ok(result);
                    }
                    Some(threshold) => {
                        self.runtime.tracer().emit(TraceEvent::ProgressiveEscalation {
                            ts_ms: corvid_runtime::now_ms(),
                            run_id: self.runtime.tracer().run_id().to_string(),
                            prompt: callee_name.to_string(),
                            from_stage: stage_index,
                            to_stage: stage_index + 1,
                            confidence_observed: result.confidence,
                            threshold,
                        });
                    }
                }
            }
            unreachable!("progressive prompt has at least one stage")
        } else {
            let selected_model = self
                .select_prompt_model(prompt, callee_name, &rendered, arg_values, span)
                .await?;
            self.execute_prompt_call(
                prompt,
                callee_name,
                arg_values,
                &rendered,
                selected_model,
                span,
            )
            .await
        }
    }

    pub(super) async fn dispatch_prompt_expr(
        &mut self,
        def_id: DefId,
        callee_name: &str,
        arg_values: &[Value],
        span: Span,
    ) -> Result<ExprFlow, InterpError> {
        let prompt = self.prompt_by_id(def_id, callee_name, span)?;
        let result = self.dispatch_prompt(prompt, callee_name, arg_values, span).await?;
        let result = self
            .maybe_escalate_stream_result(prompt, callee_name, arg_values, result, span)
            .await?;
        if !result.cost_charged && !matches!(&prompt.return_ty, Type::Stream(_)) {
            self.charge_cost(result.cost, span)?;
        }
        self.finalize_prompt_result(prompt, callee_name, arg_values, result, span).await
    }

    pub(super) async fn resume_prompt_stream(
        &mut self,
        prompt_def_id: DefId,
        prompt_name: &str,
        token: ResumeTokenValue,
        span: Span,
    ) -> Result<ExprFlow, InterpError> {
        let prompt = self.prompt_by_id(prompt_def_id, prompt_name, span)?;
        if token.prompt_name != prompt_name {
            return Err(InterpError::new(
                InterpErrorKind::DispatchFailed(format!(
                    "resume token is for prompt `{}`, not `{prompt_name}`",
                    token.prompt_name
                )),
                span,
            ));
        }

        let base_rendered = render_prompt(prompt, &token.args);
        let delivered = token
            .delivered
            .iter()
            .map(|chunk| trace_text(&value_to_json(&chunk.value)))
            .collect::<Vec<_>>()
            .join("\n");
        let continuation_rendered = if delivered.is_empty() {
            format!("{base_rendered}\n\nResume from interruption with no delivered elements.")
        } else {
            format!("{base_rendered}\n\nResume after delivered elements:\n{delivered}")
        };
        let selected_model = self
            .select_prompt_model(prompt, prompt_name, &continuation_rendered, &token.args, span)
            .await?;
        let result = self
            .execute_prompt_call(
                prompt,
                prompt_name,
                &token.args,
                &continuation_rendered,
                selected_model,
                span,
            )
            .await?;
        self.finalize_prompt_result(prompt, prompt_name, &token.args, result, span)
            .await
    }
}

fn render_prompt(prompt: &IrPrompt, args: &[Value]) -> String {
    let mut out = prompt.template.clone();
    for (param, value) in prompt.params.iter().zip(args) {
        let needle = format!("{{{}}}", param.name);
        if out.contains(&needle) {
            let replacement = value_to_json(value).to_string();
            out = out.replace(&needle, &replacement);
        }
    }
    out
}
