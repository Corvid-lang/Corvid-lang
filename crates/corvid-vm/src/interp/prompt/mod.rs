use super::{ExprFlow, Interpreter};

mod adversarial;
mod cost;
mod route_dispatch;
mod voting;
use crate::errors::{InterpError, InterpErrorKind};
use crate::value::{StreamChunk, StreamResumeContext, Value};
use crate::value_to_json;
use async_recursion::async_recursion;
use corvid_ast::Span;
use corvid_ir::IrPrompt;
use corvid_runtime::{trace_text, TraceEvent};
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
        } else if prompt.adversarial.is_some() {
            self.dispatch_adversarial_prompt(
                prompt,
                callee_name,
                arg_values,
                rendered.clone(),
                span,
            )
            .await
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
