use crate::conv::json_to_value;
use crate::errors::{InterpError, InterpErrorKind};
use crate::interp::Interpreter;
use crate::step::{StepAction, StepEvent};
use crate::value::{value_confidence, Value};
use crate::value_to_json;
use corvid_ast::Span;
use corvid_ir::IrPrompt;
use corvid_runtime::{LlmRequest, TokenUsage, TraceEvent};
use corvid_types::Type;

use super::{PromptCallResult, DEFAULT_COMPLETION_TOKEN_ESTIMATE};

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

    pub(super) fn select_named_prompt_model(
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
                    InterpErrorKind::Runtime(
                        corvid_runtime::RuntimeError::ModelOutputFormatMismatch {
                            prompt: callee_name.to_string(),
                            model: selection.model.clone(),
                            required_output_format: required.to_string(),
                            model_output_format: selection.output_format_picked.clone(),
                        },
                    ),
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

    pub(super) async fn select_prompt_model(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        rendered: &str,
        arg_values: &[Value],
        span: Span,
    ) -> Result<Option<String>, InterpError> {
        let prompt_tokens = super::super::effect_compose::estimate_tokens(rendered);
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

    pub(super) fn prompt_call_cost(
        &self,
        prompt: &IrPrompt,
        model_name: &str,
        rendered: &str,
        usage: TokenUsage,
    ) -> f64 {
        let prompt_tokens = if usage.prompt_tokens > 0 {
            usage.prompt_tokens as u64
        } else {
            super::super::effect_compose::estimate_tokens(rendered)
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

    pub(super) fn decode_prompt_response(
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
                super::super::effect_compose::composed_confidence(arg_values),
            ))
        } else {
            value
        };
        let value = response_confidence
            .map(|confidence| {
                let combined = confidence.min(value_confidence(&value));
                super::super::effect_compose::with_value_confidence(value.clone(), combined)
            })
            .unwrap_or(value);

        let confidence = super::super::effect_compose::prompt_effective_confidence(prompt, &value);
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
            super::super::effect_compose::estimate_tokens(&value_to_json(&value).to_string())
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

    pub(super) async fn execute_prompt_call(
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
                    input_confidence: super::super::effect_compose::composed_confidence(arg_values),
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
                        InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                        span,
                    )
                })?;
                let confidence =
                    super::super::effect_compose::prompt_effective_confidence(prompt, &value);
                let tokens = super::super::effect_compose::estimate_tokens(
                    &value_to_json(&value).to_string(),
                );
                let model_name = selected_model
                    .clone()
                    .unwrap_or_else(|| self.runtime.default_model().to_string());
                let cost = self.prompt_call_cost(
                    prompt,
                    &model_name,
                    rendered,
                    TokenUsage {
                        prompt_tokens: super::super::effect_compose::estimate_tokens(rendered)
                            as u32,
                        completion_tokens: tokens as u32,
                        total_tokens: (super::super::effect_compose::estimate_tokens(rendered)
                            + tokens) as u32,
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
                    result_confidence: prompt.effect_confidence.min(
                        super::super::effect_compose::composed_confidence(arg_values),
                    ),
                    elapsed_ms,
                    span,
                })
                .await?;
            if let StepAction::Override(val) = action {
                let value = json_to_value(val, result_ty, &self.types_by_id).map_err(|e| {
                    InterpError::new(
                        InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                        span,
                    )
                })?;
                let confidence =
                    super::super::effect_compose::prompt_effective_confidence(prompt, &value);
                let tokens = super::super::effect_compose::estimate_tokens(
                    &value_to_json(&value).to_string(),
                );
                let cost = self.prompt_call_cost(
                    prompt,
                    &actual_model,
                    rendered,
                    TokenUsage {
                        prompt_tokens: super::super::effect_compose::estimate_tokens(rendered)
                            as u32,
                        completion_tokens: tokens as u32,
                        total_tokens: (super::super::effect_compose::estimate_tokens(rendered)
                            + tokens) as u32,
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
}
