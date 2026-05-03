use crate::errors::{InterpError, InterpErrorKind};
use crate::interp::{ExprFlow, Interpreter};
use crate::value::{ResumeTokenValue, Value};
use crate::value_to_json;
use corvid_ast::Span;
use corvid_ir::{IrPrompt, IrRoutePattern};
use corvid_resolve::DefId;
use corvid_runtime::trace_text;
use corvid_types::Type;

use super::render_prompt;

impl<'ir> Interpreter<'ir> {
    pub(super) async fn select_prompt_route_model(
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
                    super::super::expr::require_bool(&guard_value, expr.span, "route guard")?
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

    pub(super) fn prompt_by_id(
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

    pub(in crate::interp) async fn dispatch_prompt_expr(
        &mut self,
        def_id: DefId,
        callee_name: &str,
        arg_values: &[Value],
        span: Span,
    ) -> Result<ExprFlow, InterpError> {
        let prompt = self.prompt_by_id(def_id, callee_name, span)?;
        let result = self
            .dispatch_prompt(prompt, callee_name, arg_values, span)
            .await?;
        let result = self
            .maybe_escalate_stream_result(prompt, callee_name, arg_values, result, span)
            .await?;
        if !result.cost_charged && !matches!(&prompt.return_ty, Type::Stream(_)) {
            self.charge_cost(result.cost, span)?;
        }
        self.finalize_prompt_result(prompt, callee_name, arg_values, result, span)
            .await
    }

    pub(in crate::interp) async fn resume_prompt_stream(
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
            .select_prompt_model(
                prompt,
                prompt_name,
                &continuation_rendered,
                &token.args,
                span,
            )
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
