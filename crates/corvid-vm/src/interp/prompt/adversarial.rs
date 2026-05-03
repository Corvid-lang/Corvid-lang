use crate::conv::json_to_value;
use crate::errors::{InterpError, InterpErrorKind};
use crate::interp::Interpreter;
use crate::step::{StepAction, StepEvent};
use crate::value::Value;
use crate::value_to_json;
use corvid_ast::Span;
use corvid_ir::IrPrompt;
use corvid_runtime::{contradiction_flag, trace_text, TraceEvent};
use corvid_types::Type;

use super::PromptCallResult;

impl<'ir> Interpreter<'ir> {
    pub(super) async fn dispatch_adversarial_prompt(
        &mut self,
        prompt: &'ir IrPrompt,
        callee_name: &str,
        arg_values: &[Value],
        rendered: String,
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        let spec = prompt
            .adversarial
            .as_ref()
            .expect("adversarial prompt strategy must be present");
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
                    confidence: super::super::effect_compose::prompt_effective_confidence(
                        prompt, &value,
                    ),
                    tokens: super::super::effect_compose::estimate_tokens(
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
    }
}
