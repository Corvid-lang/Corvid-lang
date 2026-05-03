use crate::conv::json_to_value;
use crate::errors::{InterpError, InterpErrorKind};
use crate::interp::Interpreter;
use crate::step::{StepAction, StepEvent};
use crate::value::Value;
use crate::value_to_json;
use corvid_ast::Span;
use corvid_ir::{IrEnsembleWeighting, IrPrompt};
use corvid_runtime::{majority_vote, weighted_vote, LlmRequest, TraceEvent};
use corvid_types::Type;
use tokio::task::JoinSet;

use super::{PromptCallResult, DEFAULT_COMPLETION_TOKEN_ESTIMATE};

impl<'ir> Interpreter<'ir> {
    pub(super) async fn dispatch_ensemble_prompt(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        rendered: String,
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        let spec = prompt
            .ensemble
            .as_ref()
            .expect("ensemble prompt strategy must be present");
        if self.should_yield_boundary() {
            let action = self
                .maybe_yield(StepEvent::BeforePromptCall {
                    prompt_name: callee_name.to_string(),
                    rendered: rendered.clone(),
                    model: None,
                    input_confidence: super::super::effect_compose::composed_confidence(arg_values),
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
                    confidence: super::super::effect_compose::prompt_effective_confidence(
                        prompt, &value,
                    ),
                    tokens: super::super::effect_compose::estimate_tokens(
                        &value_to_json(&value).to_string(),
                    ),
                    cost: prompt.effect_cost,
                    value,
                    cost_charged: false,
                });
            }
        }

        let prompt_tokens = super::super::effect_compose::estimate_tokens(&rendered);
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
        let members: Vec<String> = member_results
            .iter()
            .map(|(model, _)| model.clone())
            .collect();
        let results: Vec<String> = member_results
            .iter()
            .map(|(_, result)| super::super::effect_compose::vote_text(&result.value))
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
        let (winner_value, final_confidence) = if disagreed {
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
                    super::super::effect_compose::with_value_confidence(
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
                super::super::effect_compose::with_value_confidence(
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
                    confidence: super::super::effect_compose::prompt_effective_confidence(
                        prompt, &value,
                    ),
                    tokens: super::super::effect_compose::estimate_tokens(
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
    }
}
