use super::{ExprFlow, Interpreter};

mod cost;
mod route_dispatch;
mod voting;
use crate::conv::json_to_value;
use crate::errors::{InterpError, InterpErrorKind};
use crate::step::{StepAction, StepEvent};
use crate::value::{StreamChunk, StreamResumeContext, Value};
use crate::value_to_json;
use async_recursion::async_recursion;
use corvid_ast::Span;
use corvid_ir::IrPrompt;
use corvid_runtime::{contradiction_flag, trace_text, TraceEvent};
use corvid_types::Type;

const DEFAULT_COMPLETION_TOKEN_ESTIMATE: u64 = 256;

struct PromptCallResult {
    value: Value,
    cost: f64,
    confidence: f64,
    tokens: u64,
    cost_charged: bool,
}

impl<'ir> Interpreter<'ir> {
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

    #[async_recursion]
    async fn dispatch_prompt(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        let rendered = render_prompt(prompt, arg_values);
        if prompt.ensemble.is_some() {
            self.dispatch_ensemble_prompt(prompt, callee_name, arg_values, rendered.clone(), span)
                .await
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
                    let value =
                        json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(|e| {
                            InterpError::new(
                                InterpErrorKind::Marshal(format!(
                                    "prompt `{callee_name}` override: {e}"
                                )),
                                span,
                            )
                        })?;
                    return Ok(PromptCallResult {
                        confidence: super::effect_compose::prompt_effective_confidence(
                            prompt, &value,
                        ),
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
            let challenger =
                self.prompt_by_id(spec.challenger_def_id, &spec.challenger_name, span)?;
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
                self.runtime
                    .tracer()
                    .emit(TraceEvent::AdversarialContradiction {
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
                    let value =
                        json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(|e| {
                            InterpError::new(
                                InterpErrorKind::Marshal(format!(
                                    "prompt `{callee_name}` override: {e}"
                                )),
                                span,
                            )
                        })?;
                    return Ok(PromptCallResult {
                        confidence: super::effect_compose::prompt_effective_confidence(
                            prompt, &value,
                        ),
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
                            self.runtime
                                .tracer()
                                .emit(TraceEvent::ProgressiveExhausted {
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
                        self.runtime
                            .tracer()
                            .emit(TraceEvent::ProgressiveEscalation {
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
