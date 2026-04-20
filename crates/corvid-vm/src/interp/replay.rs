use super::{ExprFlow, Interpreter};
use crate::conv::json_to_value;
use crate::errors::{InterpError, InterpErrorKind};
use crate::value::Value;
use corvid_ast::Span;
use corvid_ir::{IrExpr, IrReplayArm, IrReplayCapture, IrReplayPattern, IrReplayToolArgPattern};
use corvid_runtime::replay_dispatch::{
    find_first_replay_match, ReplayDispatchArm, ReplayDispatchPattern, ReplayDispatchToolArgPattern,
};
use corvid_runtime::WRITER_INTERPRETER;
use corvid_types::Type;

impl<'ir> Interpreter<'ir> {
    pub(super) async fn eval_replay_expr(
        &mut self,
        trace: &'ir IrExpr,
        arms: &'ir [IrReplayArm],
        else_body: &'ir IrExpr,
        span: Span,
    ) -> Result<ExprFlow, InterpError> {
        let trace_value = match self.eval_expr(trace).await?.into_value() {
            Ok(value) => value,
            Err(value) => return Ok(ExprFlow::Propagate(value)),
        };
        let trace_path = match trace_value {
            Value::String(path) => path.to_string(),
            other => {
                return Err(InterpError::new(
                    InterpErrorKind::TypeMismatch {
                        expected: "TraceId".into(),
                        got: other.type_name().into(),
                    },
                    trace.span,
                ));
            }
        };
        let dispatch_arms: Vec<ReplayDispatchArm> =
            arms.iter().map(Self::lower_replay_dispatch_arm).collect();
        let matched =
            find_first_replay_match(&trace_path, WRITER_INTERPRETER, &dispatch_arms).map_err(
                |err| InterpError::new(InterpErrorKind::Runtime(err), span),
            )?;
        let Some(found) = matched else {
            return self.eval_expr(else_body).await;
        };
        let arm = &arms[found.arm_index];
        self.eval_replay_arm_with_bindings(arm, found.whole_value, found.tool_arg_value)
            .await
    }

    fn lower_replay_dispatch_arm(arm: &IrReplayArm) -> ReplayDispatchArm {
        let pattern = match &arm.pattern {
            IrReplayPattern::Llm { prompt, .. } => ReplayDispatchPattern::Llm {
                prompt: prompt.clone(),
            },
            IrReplayPattern::Tool { tool, arg, .. } => ReplayDispatchPattern::Tool {
                tool: tool.clone(),
                arg: match arg {
                    IrReplayToolArgPattern::Wildcard => ReplayDispatchToolArgPattern::Wildcard,
                    IrReplayToolArgPattern::StringLit(value) => {
                        ReplayDispatchToolArgPattern::StringLit(value.clone())
                    }
                    IrReplayToolArgPattern::Capture(_) => ReplayDispatchToolArgPattern::Capture,
                },
            },
            IrReplayPattern::Approve { label, .. } => ReplayDispatchPattern::Approve {
                label: label.clone(),
            },
        };
        ReplayDispatchArm { pattern }
    }

    async fn eval_replay_arm_with_bindings(
        &mut self,
        arm: &'ir IrReplayArm,
        whole_value_json: serde_json::Value,
        tool_arg_json: Option<serde_json::Value>,
    ) -> Result<ExprFlow, InterpError> {
        let saved_env = self.env.clone();
        let saved_names = self.local_names.clone();

        if let Some(capture) = &arm.capture {
            let ty = self.replay_whole_capture_type(&arm.pattern)?;
            let value = json_to_value(whole_value_json, &ty, &self.types_by_id).map_err(|err| {
                InterpError::new(InterpErrorKind::Marshal(err.to_string()), capture.span)
            })?;
            self.env.bind(capture.local_id, value);
            self.local_names.insert(capture.local_id, capture.name.clone());
        }

        if let IrReplayPattern::Tool {
            tool,
            arg: IrReplayToolArgPattern::Capture(capture),
            ..
        } = &arm.pattern
        {
            let raw = tool_arg_json.ok_or_else(|| {
                InterpError::new(
                    InterpErrorKind::Other(format!(
                        "replay tool arm for `{tool}` matched without a captured first arg"
                    )),
                    capture.span,
                )
            })?;
            let ty = self.replay_tool_arg_capture_type(tool, capture)?;
            let value = json_to_value(raw, &ty, &self.types_by_id).map_err(|err| {
                InterpError::new(InterpErrorKind::Marshal(err.to_string()), capture.span)
            })?;
            self.env.bind(capture.local_id, value);
            self.local_names.insert(capture.local_id, capture.name.clone());
        }

        let outcome = self.eval_expr(&arm.body).await;
        self.env = saved_env;
        self.local_names = saved_names;
        outcome
    }

    fn replay_whole_capture_type(&self, pattern: &IrReplayPattern) -> Result<Type, InterpError> {
        match pattern {
            IrReplayPattern::Llm { prompt, span } => self
                .ir
                .prompts
                .iter()
                .find(|candidate| candidate.name == *prompt)
                .map(|candidate| candidate.return_ty.clone())
                .ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::DispatchFailed(format!(
                            "no prompt named `{prompt}` for replay capture"
                        )),
                        *span,
                    )
                }),
            IrReplayPattern::Tool { tool, span, .. } => self
                .ir
                .tools
                .iter()
                .find(|candidate| candidate.name == *tool)
                .map(|candidate| candidate.return_ty.clone())
                .ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::DispatchFailed(format!(
                            "no tool named `{tool}` for replay capture"
                        )),
                        *span,
                    )
                }),
            IrReplayPattern::Approve { .. } => Ok(Type::Bool),
        }
    }

    fn replay_tool_arg_capture_type(
        &self,
        tool: &str,
        capture: &IrReplayCapture,
    ) -> Result<Type, InterpError> {
        let tool_decl = self
            .ir
            .tools
            .iter()
            .find(|candidate| candidate.name == tool)
            .ok_or_else(|| {
                InterpError::new(
                    InterpErrorKind::DispatchFailed(format!(
                        "no tool named `{tool}` for replay arg capture"
                    )),
                    capture.span,
                )
            })?;
        tool_decl
            .params
            .first()
            .map(|param| param.ty.clone())
            .ok_or_else(|| {
                InterpError::new(
                    InterpErrorKind::DispatchFailed(format!(
                        "tool `{tool}` has no first parameter for replay arg capture"
                    )),
                    capture.span,
                )
            })
    }
}
