//! Tree-walking interpreter, async edition.
//!
//! Asynchronous from the top because tool calls, prompt calls, and
//! approvals are async at the runtime boundary. The performance hit of
//! boxing recursive futures (via `async-recursion`) is the price for
//! keeping this tier behaviourally identical to the future Cranelift
//! backend, which will also be async-native. Behavioural parity is what
//! makes this interpreter useful as a correctness oracle.

#[path = "interp/effect_compose.rs"]
mod effect_compose;
#[path = "interp/expr.rs"]
mod expr;

use crate::conv::{json_to_value, value_to_json};
use crate::env::Env;
use crate::errors::{InterpError, InterpErrorKind};
use crate::step::{self, StepAction, StepController, StepEvent, StepMode, StmtKind};
use crate::value::{BoxedValue, ListValue, StreamChunk, StreamSender, StreamValue, StructValue, Value};
use self::expr::{eval_binop, eval_literal, eval_unop, require_bool};
use effect_compose::{
    citation_verified, composed_confidence, default_stream_backpressure, estimate_tokens,
    prompt_backpressure, prompt_effective_confidence, stream_start_is_retryable, vote_text,
    with_value_confidence,
};
use async_recursion::async_recursion;
use corvid_ast::{BackpressurePolicy, BinaryOp, Span};
use corvid_ir::{
    IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrPrompt, IrRoutePattern, IrStmt,
    IrTool, IrType,
};
use corvid_resolve::{DefId, LocalId};
use corvid_runtime::{
    contradiction_flag, majority_vote, trace_text, LlmRequest, Runtime, TokenUsage, TraceEvent,
};
use corvid_types::Type;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinSet;

const DEFAULT_COMPLETION_TOKEN_ESTIMATE: u64 = 256;

/// Public entry point: run `agent_name` with `args` against `runtime`.
///
/// The runtime owns tool/LLM/approval dispatch and tracing. Pass a
/// minimal runtime built via `Runtime::builder().build()` for tests
/// that don't exercise external calls.
pub async fn run_agent(
    ir: &IrFile,
    agent_name: &str,
    args: Vec<Value>,
    runtime: &Runtime,
) -> Result<Value, InterpError> {
    run_agent_with_env(ir, agent_name, args, runtime)
        .await
        .map(|(value, _env)| value)
}

pub async fn run_agent_with_env(
    ir: &IrFile,
    agent_name: &str,
    args: Vec<Value>,
    runtime: &Runtime,
) -> Result<(Value, Env), InterpError> {
    let agent = ir
        .agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            InterpError::new(
                InterpErrorKind::DispatchFailed(format!(
                    "no agent named `{agent_name}`"
                )),
                Span::new(0, 0),
            )
        })?;

    runtime.tracer().emit(TraceEvent::RunStarted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        agent: agent_name.to_string(),
        args: args.iter().map(value_to_json).collect(),
    });

    let mut interp = Interpreter::new(ir, runtime);
    let bind_result = interp.bind_params(agent, args);
    let outcome = match bind_result {
        Ok(()) => interp.run_body(agent).await.map(|value| (value, interp.env.clone())),
        Err(e) => Err(e),
    };

    runtime.tracer().emit(TraceEvent::RunCompleted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        ok: outcome.is_ok(),
        result: outcome
            .as_ref()
            .ok()
            .map(|(value, _env)| value_to_json(value)),
        error: outcome.as_ref().err().map(|error| error.to_string()),
    });
    outcome
}

/// Run an agent with step-through control. The `hook` receives events at
/// tool/prompt/approval/agent-call boundaries (and optionally at every
/// statement) and decides whether to continue, override, or abort.
pub async fn run_agent_stepping(
    ir: &IrFile,
    agent_name: &str,
    args: Vec<Value>,
    runtime: &Runtime,
    hook: Arc<dyn crate::step::StepHook>,
    mode: StepMode,
) -> Result<(Value, Env), InterpError> {
    let agent = ir
        .agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            InterpError::new(
                InterpErrorKind::DispatchFailed(format!("no agent named `{agent_name}`")),
                Span::new(0, 0),
            )
        })?;

    runtime.tracer().emit(TraceEvent::RunStarted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        agent: agent_name.to_string(),
        args: args.iter().map(value_to_json).collect(),
    });

    let mut interp = Interpreter::new(ir, runtime);
    interp.stepper = Some(StepController::new(hook, mode));
    let bind_result = interp.bind_params(agent, args);
    let outcome = match bind_result {
        Ok(()) => interp.run_body(agent).await.map(|value| (value, interp.env.clone())),
        Err(e) => Err(e),
    };

    let _ = interp.maybe_yield(StepEvent::Completed {
        agent_name: agent_name.to_string(),
        ok: outcome.is_ok(),
        result: outcome.as_ref().ok().map(|(v, _)| v.clone()),
        error: outcome.as_ref().err().map(|e| e.to_string()),
    }).await;

    runtime.tracer().emit(TraceEvent::RunCompleted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        ok: outcome.is_ok(),
        result: outcome.as_ref().ok().map(|(value, _)| value_to_json(value)),
        error: outcome.as_ref().err().map(|error| error.to_string()),
    });
    outcome
}

/// Pre-bind specific locals and run an agent. Used by tests that want
/// to inject pre-built struct parameters bypassing the parameter list.
pub async fn bind_and_run_agent(
    ir: &IrFile,
    agent_name: &str,
    params_with_values: Vec<(LocalId, Value)>,
    fallback_args: Vec<Value>,
    runtime: &Runtime,
) -> Result<Value, InterpError> {
    if params_with_values.is_empty() {
        return run_agent(ir, agent_name, fallback_args, runtime).await;
    }
    let agent = ir
        .agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            InterpError::new(
                InterpErrorKind::DispatchFailed(format!("no agent named `{agent_name}`")),
                Span::new(0, 0),
            )
        })?;
    let mut interp = Interpreter::new(ir, runtime);
    for (id, v) in params_with_values {
        interp.env.bind(id, v);
    }
    interp.run_body(agent).await
}

/// Build a struct `Value` from field name → value pairs. Convenience used
/// by tests to construct struct arguments to inject into agent runs.
pub fn build_struct(
    type_id: DefId,
    type_name: &str,
    fields: impl IntoIterator<Item = (String, Value)>,
) -> Value {
    Value::Struct(StructValue::new(type_id, type_name.to_string(), fields))
}

/// Control-flow outcome of evaluating a statement or block.
#[derive(Debug, Clone)]
enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

#[derive(Debug, Clone)]
enum ExprFlow {
    Value(Value),
    Propagate(Value),
}

impl ExprFlow {
    fn into_value(self) -> Result<Value, Value> {
        match self {
            ExprFlow::Value(v) => Ok(v),
            ExprFlow::Propagate(v) => Err(v),
        }
    }
}

struct PromptCallResult {
    value: Value,
    cost: f64,
    confidence: f64,
    tokens: u64,
    cost_charged: bool,
}

struct Interpreter<'ir> {
    ir: &'ir IrFile,
    env: Env,
    types_by_id: HashMap<DefId, &'ir IrType>,
    tools_by_id: HashMap<DefId, &'ir IrTool>,
    prompts_by_id: HashMap<DefId, &'ir IrPrompt>,
    agents_by_id: HashMap<DefId, &'ir IrAgent>,
    runtime: &'ir Runtime,
    local_names: HashMap<LocalId, String>,
    stepper: Option<StepController>,
    stream_sender: Option<StreamSender>,
    stream_locals: HashMap<LocalId, StreamChunk>,
    cost_budget: Option<f64>,
    cost_used: f64,
    stream_cost_budget: Option<f64>,
    stream_cost_used: f64,
}

impl<'ir> Interpreter<'ir> {
    fn new(ir: &'ir IrFile, runtime: &'ir Runtime) -> Self {
        let types_by_id: HashMap<DefId, &IrType> =
            ir.types.iter().map(|t| (t.id, t)).collect();
        let tools_by_id: HashMap<DefId, &IrTool> =
            ir.tools.iter().map(|t| (t.id, t)).collect();
        let prompts_by_id: HashMap<DefId, &IrPrompt> =
            ir.prompts.iter().map(|p| (p.id, p)).collect();
        let agents_by_id: HashMap<DefId, &IrAgent> =
            ir.agents.iter().map(|a| (a.id, a)).collect();
        Self {
            ir,
            env: Env::new(),
            types_by_id,
            tools_by_id,
            prompts_by_id,
            agents_by_id,
            runtime,
            local_names: HashMap::new(),
            stepper: None,
            stream_sender: None,
            stream_locals: HashMap::new(),
            cost_budget: None,
            cost_used: 0.0,
            stream_cost_budget: None,
            stream_cost_used: 0.0,
        }
    }

    fn bind_params(
        &mut self,
        agent: &'ir IrAgent,
        args: Vec<Value>,
    ) -> Result<(), InterpError> {
        if agent.params.len() != args.len() {
            return Err(InterpError::new(
                InterpErrorKind::DispatchFailed(format!(
                    "agent `{}` expects {} arg(s), got {}",
                    agent.name,
                    agent.params.len(),
                    args.len()
                )),
                agent.span,
            ));
        }
        for (p, v) in agent.params.iter().zip(args) {
            self.env.bind(p.local_id, v.clone());
            self.local_names.insert(p.local_id, p.name.clone());
            self.stream_locals.remove(&p.local_id);
        }
        Ok(())
    }

    fn env_snapshot(&self) -> step::EnvSnapshot {
        step::snapshot_env(&self.env, &self.local_names)
    }

    fn emit_model_selected(
        &self,
        callee_name: &str,
        model: String,
        capability_required: Option<String>,
        capability_picked: Option<String>,
        cost_estimate: f64,
        arm_index: Option<usize>,
        stage_index: Option<usize>,
    ) {
        self.runtime.tracer().emit(TraceEvent::ModelSelected {
            ts_ms: corvid_runtime::now_ms(),
            run_id: self.runtime.tracer().run_id().to_string(),
            prompt: callee_name.to_string(),
            model,
            capability_required,
            capability_picked,
            cost_estimate,
            arm_index,
            stage_index,
        });
    }

    fn select_named_prompt_model(
        &self,
        callee_name: &str,
        model_name: &str,
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
        self.emit_model_selected(
            callee_name,
            selection.model.clone(),
            selection.capability_required,
            selection.capability_picked,
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
        let prompt_tokens = estimate_tokens(rendered);
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

        let Some(required) = prompt.capability_required.as_deref() else {
            return Ok(None);
        };
        let selection = self.runtime.select_cheapest_model_for_capability(
            required,
            prompt_tokens,
            completion_tokens,
        ).map_err(|err| InterpError::new(InterpErrorKind::Runtime(err), span))?;
        self.emit_model_selected(
            callee_name,
            selection.model.clone(),
            selection.capability_required,
            selection.capability_picked,
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
                    require_bool(&guard_value, expr.span, "route guard")?
                }
            };
            if !matched {
                continue;
            }
            return self
                .select_named_prompt_model(
                    callee_name,
                    &arm.model_name,
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
            estimate_tokens(rendered)
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
        span: Span,
    ) -> Result<PromptCallResult, InterpError> {
        let result_ty = match &prompt.return_ty {
            Type::Stream(inner) => inner.as_ref(),
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
                let ctx_text = value_to_json(ctx_value).to_string();
                let response_text = value_to_json(&value).to_string();
                if !citation_verified(&ctx_text, &response_text) {
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

        let mut merged_chain = crate::value::ProvenanceChain::new();
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
                composed_confidence(arg_values),
            ))
        } else {
            value
        };

        let confidence = prompt_effective_confidence(prompt, &value);
        let tokens = if usage.completion_tokens > 0 {
            usage.completion_tokens as u64
        } else if usage.total_tokens > 0 {
            usage.total_tokens as u64
        } else {
            estimate_tokens(&value_to_json(&value).to_string())
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
            let action = self.maybe_yield(StepEvent::BeforePromptCall {
                prompt_name: callee_name.to_string(),
                rendered: rendered.to_string(),
                model: selected_model.clone(),
                span,
                env: self.env_snapshot(),
            }).await?;
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
                let confidence = prompt_effective_confidence(prompt, &value);
                let tokens = estimate_tokens(&value_to_json(&value).to_string());
                let model_name = selected_model
                    .clone()
                    .unwrap_or_else(|| self.runtime.default_model().to_string());
                let cost = self.prompt_call_cost(
                    prompt,
                    &model_name,
                    rendered,
                    TokenUsage {
                        prompt_tokens: estimate_tokens(rendered) as u32,
                        completion_tokens: tokens as u32,
                        total_tokens: (estimate_tokens(rendered) + tokens) as u32,
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
            .call_llm(req)
            .await
            .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        if self.should_yield_boundary() {
            let action = self.maybe_yield(StepEvent::AfterPromptCall {
                prompt_name: callee_name.to_string(),
                result: resp.value.clone(),
                elapsed_ms,
                span,
            }).await?;
            if let StepAction::Override(val) = action {
                let value = json_to_value(val, result_ty, &self.types_by_id).map_err(|e| {
                    InterpError::new(
                        InterpErrorKind::Marshal(format!(
                            "prompt `{callee_name}` override: {e}"
                        )),
                        span,
                    )
                })?;
                let confidence = prompt_effective_confidence(prompt, &value);
                let tokens = estimate_tokens(&value_to_json(&value).to_string());
                let cost = self.prompt_call_cost(
                    prompt,
                    &actual_model,
                    rendered,
                    TokenUsage {
                        prompt_tokens: estimate_tokens(rendered) as u32,
                        completion_tokens: tokens as u32,
                        total_tokens: (estimate_tokens(rendered) + tokens) as u32,
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
            span,
        )
    }

    async fn finalize_prompt_result(
        &self,
        prompt: &'ir IrPrompt,
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
                            prompt_backpressure(prompt),
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
                            prompt_backpressure(prompt),
                        )
                        .await
                        .map(ExprFlow::Value);
                }
            }
            Ok(ExprFlow::Value(
                self.singleton_stream(chunk, prompt_backpressure(prompt)).await?,
            ))
        } else {
            Ok(ExprFlow::Value(result.value))
        }
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
                        span,
                        env: self.env_snapshot(),
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(|e| {
                        InterpError::new(
                            InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                            span,
                        )
                    })?;
                    return Ok(PromptCallResult {
                        confidence: prompt_effective_confidence(prompt, &value),
                        tokens: estimate_tokens(&value_to_json(&value).to_string()),
                        cost: prompt.effect_cost,
                        value,
                        cost_charged: false,
                    });
                }
            }

            let prompt_tokens = estimate_tokens(&rendered);
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
                join_set.spawn(async move {
                    let response = runtime.call_llm(req).await;
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
                .map(|(_, result)| vote_text(&result.value))
                .collect();
            let vote = majority_vote(&results);
            let winner_index = results
                .iter()
                .position(|result| result == &vote.winner)
                .expect("winner must be one of the results");
            let total_cost: f64 = member_results.iter().map(|(_, result)| result.cost).sum();
            let total_tokens: u64 =
                member_results.iter().map(|(_, result)| result.tokens).sum();
            let min_confidence = member_results
                .iter()
                .map(|(_, result)| result.confidence)
                .fold(1.0_f64, f64::min);
            let combined_confidence = min_confidence * vote.agreement_rate;
            let winner_value = with_value_confidence(
                member_results[winner_index].1.value.clone(),
                combined_confidence,
            );

            self.runtime.tracer().emit(TraceEvent::EnsembleVote {
                ts_ms: corvid_runtime::now_ms(),
                run_id: self.runtime.tracer().run_id().to_string(),
                prompt: callee_name.to_string(),
                members,
                results: results.clone(),
                winner: vote.winner.clone(),
                agreement_rate: vote.agreement_rate,
                strategy: "majority".to_string(),
            });

            if self.should_yield_boundary() {
                let action = self
                    .maybe_yield(StepEvent::AfterPromptCall {
                        prompt_name: callee_name.to_string(),
                        result: value_to_json(&winner_value),
                        elapsed_ms: ensemble_start.elapsed().as_millis() as u64,
                        span,
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(|e| {
                        InterpError::new(
                            InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                            span,
                        )
                    })?;
                    return Ok(PromptCallResult {
                        confidence: prompt_effective_confidence(prompt, &value),
                        tokens: estimate_tokens(&value_to_json(&value).to_string()),
                        cost: total_cost,
                        value,
                        cost_charged: false,
                    });
                }
            }

            Ok(PromptCallResult {
                value: winner_value,
                cost: total_cost,
                confidence: combined_confidence,
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
                        span,
                        env: self.env_snapshot(),
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(|e| {
                        InterpError::new(
                            InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                            span,
                        )
                    })?;
                    return Ok(PromptCallResult {
                        confidence: prompt_effective_confidence(prompt, &value),
                        tokens: estimate_tokens(&value_to_json(&value).to_string()),
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
                        elapsed_ms: pipeline_start.elapsed().as_millis() as u64,
                        span,
                    })
                    .await?;
                if let StepAction::Override(val) = action {
                    let value = json_to_value(val, &prompt.return_ty, &self.types_by_id).map_err(|e| {
                        InterpError::new(
                            InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                            span,
                        )
                    })?;
                    return Ok(PromptCallResult {
                        confidence: prompt_effective_confidence(prompt, &value),
                        tokens: estimate_tokens(&value_to_json(&value).to_string()),
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
            let prompt_tokens = estimate_tokens(&rendered);
            let completion_tokens = prompt
                .max_tokens
                .unwrap_or(DEFAULT_COMPLETION_TOKEN_ESTIMATE);
            let chosen_model = if self.runtime.choose_rollout_variant(spec.variant_percent) {
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
            let prompt_tokens = estimate_tokens(&rendered);
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

    async fn maybe_yield(&mut self, event: StepEvent) -> Result<StepAction, InterpError> {
        if let Some(stepper) = self.stepper.as_mut() {
            let action = stepper.yield_event(event).await;
            if matches!(action, StepAction::Abort) {
                return Err(InterpError::new(
                    InterpErrorKind::Other("execution aborted by step controller".into()),
                    Span::new(0, 0),
                ));
            }
            Ok(action)
        } else {
            Ok(StepAction::Resume)
        }
    }

    fn should_yield_statement(&self) -> bool {
        self.stepper.as_ref().is_some_and(|s| s.should_yield_on_statement())
    }

    fn should_yield_boundary(&self) -> bool {
        self.stepper.as_ref().is_some_and(|s| s.should_yield_on_boundary())
    }

    async fn run_body(&mut self, agent: &'ir IrAgent) -> Result<Value, InterpError> {
        if matches!(&agent.return_ty, Type::Stream(_)) {
            return self.spawn_stream_agent(agent).await;
        }
        let saved_budget = self.cost_budget;
        let saved_used = self.cost_used;
        self.cost_budget = agent.cost_budget;
        self.cost_used = 0.0;
        let flow = self.eval_block(&agent.body).await;
        self.cost_budget = saved_budget;
        self.cost_used = saved_used;
        match flow? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Ok(Value::Nothing),
            Flow::Break | Flow::Continue => Err(InterpError::new(
                InterpErrorKind::Other(
                    "loop control flow escaped its enclosing loop".into(),
                ),
                agent.span,
            )),
        }
    }

    async fn spawn_stream_agent(&mut self, agent: &'ir IrAgent) -> Result<Value, InterpError> {
        let (sender, stream) = StreamValue::channel(default_stream_backpressure());
        let ir = self.ir.clone();
        let runtime = self.runtime.clone();
        let agent = agent.clone();
        let env = self.env.clone();
        let local_names = self.local_names.clone();
        tokio::spawn(async move {
            let mut sub = Interpreter::new(&ir, &runtime);
            sub.env = env;
            sub.local_names = local_names;
            sub.stream_sender = Some(sender);
            sub.stream_cost_budget = agent.cost_budget;
            let outcome = sub.eval_block(&agent.body).await;
            let maybe_sender = sub.stream_sender.take();
            match outcome {
                Ok(Flow::Normal) | Ok(Flow::Return(_)) => {}
                Ok(Flow::Break) | Ok(Flow::Continue) => {
                    if let Some(sender) = maybe_sender {
                        let _ = sender.send(Err(InterpError::new(
                            InterpErrorKind::Other(
                                "loop control flow escaped its enclosing loop".into(),
                            ),
                            agent.span,
                        ))).await;
                    }
                }
                Err(err) => {
                    if let Some(sender) = maybe_sender {
                        let _ = sender.send(Err(err)).await;
                    }
                }
            }
        });
        Ok(Value::Stream(stream))
    }

    async fn singleton_stream(
        &self,
        chunk: StreamChunk,
        backpressure: BackpressurePolicy,
    ) -> Result<Value, InterpError> {
        let (sender, stream) = StreamValue::channel(backpressure);
        let _ = sender.send_chunk(Ok(chunk)).await;
        Ok(Value::Stream(stream))
    }

    async fn singleton_stream_error(
        &self,
        err: InterpError,
        backpressure: BackpressurePolicy,
    ) -> Result<Value, InterpError> {
        let (sender, stream) = StreamValue::channel(backpressure);
        let _ = sender.send_chunk(Err(err)).await;
        Ok(Value::Stream(stream))
    }

    fn prepend_stream_chunk(
        &self,
        first: StreamChunk,
        stream: StreamValue,
    ) -> Value {
        let backpressure = stream.backpressure().clone();
        let (sender, combined) = StreamValue::channel(backpressure);
        tokio::spawn(async move {
            if !sender.send_chunk(Ok(first)).await {
                return;
            }
            while let Some(item) = stream.next_chunk().await {
                if !sender.send_chunk(item).await {
                    break;
                }
            }
        });
        Value::Stream(combined)
    }

    fn chunk_for_expr(&self, expr: &IrExpr, value: Value) -> StreamChunk {
        if let IrExprKind::Local { local_id, .. } = &expr.kind {
            if let Some(chunk) = self.stream_locals.get(local_id) {
                return StreamChunk {
                    value,
                    cost: chunk.cost,
                    confidence: chunk.confidence,
                    tokens: chunk.tokens,
                };
            }
        }
        StreamChunk::new(value)
    }

    fn stream_limit_violation(&self, chunk: &StreamChunk, span: Span) -> Option<InterpError> {
        let budget = self.stream_cost_budget?;
        let used = self.stream_cost_used + chunk.cost;
        if used > budget {
            Some(InterpError::new(
                InterpErrorKind::BudgetExceeded { budget, used },
                span,
            ))
        } else {
            None
        }
    }

    fn charge_cost(&mut self, cost: f64, span: Span) -> Result<(), InterpError> {
        let Some(budget) = self.cost_budget else {
            self.cost_used += cost;
            return Ok(());
        };
        let used = self.cost_used + cost;
        if used > budget {
            return Err(InterpError::new(
                InterpErrorKind::BudgetExceeded { budget, used },
                span,
            ));
        }
        self.cost_used = used;
        Ok(())
    }

    #[async_recursion]
    async fn eval_block(&mut self, block: &'ir IrBlock) -> Result<Flow, InterpError> {
        for stmt in &block.stmts {
            match self.eval_stmt(stmt).await? {
                Flow::Normal => continue,
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    #[async_recursion]
    async fn eval_stmt(&mut self, stmt: &'ir IrStmt) -> Result<Flow, InterpError> {
        match stmt {
            IrStmt::Let { local_id, name, value, .. } => {
                if self.should_yield_statement() {
                    self.maybe_yield(StepEvent::BeforeStatement {
                        kind: StmtKind::Let { name: name.clone() },
                        span: value.span,
                        env: self.env_snapshot(),
                    }).await?;
                }
                let v = match self.eval_expr(value).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(Flow::Return(v)),
                };
                self.env.bind(*local_id, v);
                self.local_names.insert(*local_id, name.clone());
                if let IrExprKind::Local { local_id: source_local, .. } = &value.kind {
                    if let Some(chunk) = self.stream_locals.get(source_local).cloned() {
                        self.stream_locals.insert(
                            *local_id,
                            StreamChunk { value: self.env.lookup(*local_id).unwrap_or(Value::Nothing), ..chunk },
                        );
                    } else {
                        self.stream_locals.remove(local_id);
                    }
                } else {
                    self.stream_locals.remove(local_id);
                }
                Ok(Flow::Normal)
            }
            IrStmt::Return { value, .. } => {
                let v = match value {
                    Some(e) => match self.eval_expr(e).await?.into_value() {
                        Ok(v) | Err(v) => v,
                    },
                    None => Value::Nothing,
                };
                Ok(Flow::Return(v))
            }
            IrStmt::Yield { value, span } => {
                let yielded = match self.eval_expr(value).await?.into_value() {
                    Ok(v) | Err(v) => v,
                };
                let Some(sender) = self.stream_sender.as_ref() else {
                    return Err(InterpError::new(
                        InterpErrorKind::NotImplemented("stream yield statements".into()),
                        *span,
                    ));
                };
                let chunk = self.chunk_for_expr(value, yielded);
                if let Some(err) = self.stream_limit_violation(&chunk, *span) {
                    let _ = sender.send_chunk(Err(err)).await;
                    return Ok(Flow::Return(Value::Nothing));
                }
                self.stream_cost_used += chunk.cost;
                if !sender.send_chunk(Ok(chunk)).await {
                    return Ok(Flow::Return(Value::Nothing));
                }
                Ok(Flow::Normal)
            }
            IrStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let c = match self.eval_expr(cond).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(Flow::Return(v)),
                };
                let take_then = match c {
                    Value::Bool(b) => b,
                    other => {
                        return Err(InterpError::new(
                            InterpErrorKind::TypeMismatch {
                                expected: "Bool".into(),
                                got: other.type_name(),
                            },
                            cond.span,
                        ))
                    }
                };
                if take_then {
                    self.eval_block(then_block).await
                } else if let Some(eb) = else_block {
                    self.eval_block(eb).await
                } else {
                    Ok(Flow::Normal)
                }
            }
            IrStmt::For {
                var_local,
                iter,
                body,
                span,
                ..
            } => {
                let iter_val = match self.eval_expr(iter).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(Flow::Return(v)),
                };
                match iter_val {
                    Value::List(items) => {
                        self.stream_locals.remove(var_local);
                        for item in items.iter_cloned() {
                            self.env.bind(*var_local, item);
                            match self.eval_block(body).await? {
                                Flow::Normal | Flow::Continue => continue,
                                Flow::Break => return Ok(Flow::Normal),
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                            }
                        }
                    }
                    Value::String(s) => {
                        self.stream_locals.remove(var_local);
                        for item in s.chars().map(|c| Value::String(Arc::from(c.to_string()))) {
                            self.env.bind(*var_local, item);
                            match self.eval_block(body).await? {
                                Flow::Normal | Flow::Continue => continue,
                                Flow::Break => return Ok(Flow::Normal),
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                            }
                        }
                    }
                    Value::Stream(stream) => {
                        while let Some(item) = stream.next_chunk().await {
                            let chunk = item?;
                            self.env.bind(*var_local, chunk.value.clone());
                            self.stream_locals.insert(*var_local, chunk);
                            match self.eval_block(body).await? {
                                Flow::Normal | Flow::Continue => continue,
                                Flow::Break => return Ok(Flow::Normal),
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                            }
                        }
                        self.stream_locals.remove(var_local);
                    }
                    other => {
                        return Err(InterpError::new(
                            InterpErrorKind::TypeMismatch {
                                expected: "List, Stream, or String".into(),
                                got: other.type_name(),
                            },
                            *span,
                        ))
                    }
                }
                Ok(Flow::Normal)
            }
            IrStmt::Approve { label, args, span } => {
                let mut json_args = Vec::with_capacity(args.len());
                for a in args {
                    let v = match self.eval_expr(a).await?.into_value() {
                        Ok(v) => v,
                        Err(v) => return Ok(Flow::Return(v)),
                    };
                    json_args.push(value_to_json(&v));
                }

                if self.should_yield_boundary() {
                    let action = self.maybe_yield(StepEvent::BeforeApproval {
                        label: label.clone(),
                        args: json_args.clone(),
                        span: *span,
                        env: self.env_snapshot(),
                    }).await?;

                    match action {
                        StepAction::Approve => {
                            if self.should_yield_boundary() {
                                self.maybe_yield(StepEvent::AfterApproval {
                                    label: label.clone(),
                                    approved: true,
                                    span: *span,
                                }).await?;
                            }
                            return Ok(Flow::Normal);
                        }
                        StepAction::Deny => {
                            if self.should_yield_boundary() {
                                self.maybe_yield(StepEvent::AfterApproval {
                                    label: label.clone(),
                                    approved: false,
                                    span: *span,
                                }).await?;
                            }
                            return Err(InterpError::new(
                                InterpErrorKind::Runtime(
                                    corvid_runtime::RuntimeError::ApprovalDenied {
                                        action: label.clone(),
                                    },
                                ),
                                *span,
                            ));
                        }
                        _ => {}
                    }
                }

                let result = self.runtime
                    .approval_gate(label, json_args)
                    .await;
                let approved = result.is_ok();

                if self.should_yield_boundary() {
                    self.maybe_yield(StepEvent::AfterApproval {
                        label: label.clone(),
                        approved,
                        span: *span,
                    }).await?;
                }

                result.map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), *span))?;
                Ok(Flow::Normal)
            }
            IrStmt::Expr { expr, .. } => {
                if let Err(v) = self.eval_expr(expr).await?.into_value() {
                    return Ok(Flow::Return(v));
                }
                Ok(Flow::Normal)
            }
            IrStmt::Break { .. } => Ok(Flow::Break),
            IrStmt::Continue { .. } => Ok(Flow::Continue),
            IrStmt::Pass { .. } => Ok(Flow::Normal),
            // The interpreter uses Arc for refcounting, so
            // codegen-inserted Dup/Drop are ignorable at this
            // tier. The native codegen lowers them to corvid_retain
            // / corvid_release calls.
            IrStmt::Dup { .. } | IrStmt::Drop { .. } => Ok(Flow::Normal),
        }
    }

    #[async_recursion]
    async fn eval_expr(&mut self, expr: &'ir IrExpr) -> Result<ExprFlow, InterpError> {
        match &expr.kind {
            IrExprKind::Literal(lit) => Ok(ExprFlow::Value(eval_literal(lit))),

            IrExprKind::Local { local_id, .. } => {
                self.env.lookup(*local_id).map(ExprFlow::Value).ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::UndefinedLocal(*local_id),
                        expr.span,
                    )
                })
            }

            IrExprKind::Decl { .. } => Err(InterpError::new(
                InterpErrorKind::NotImplemented(
                    "bare top-level declaration reference (imports/functions)".into(),
                ),
                expr.span,
            )),

            IrExprKind::Call { kind, callee_name, args } => {
                self.eval_call(kind, callee_name, args, &expr.ty, expr.span)
                    .await
            }

            IrExprKind::FieldAccess { target, field } => {
                let t = match self.eval_expr(target).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                match t {
                    Value::Struct(s) => s.get_field(field).map(ExprFlow::Value).ok_or_else(|| {
                        InterpError::new(
                            InterpErrorKind::UnknownField {
                                struct_name: s.type_name().to_string(),
                                field: field.clone(),
                            },
                            expr.span,
                        )
                    }),
                    other => Err(InterpError::new(
                        InterpErrorKind::TypeMismatch {
                            expected: "struct".into(),
                            got: other.type_name(),
                        },
                        expr.span,
                    )),
                }
            }

            IrExprKind::Index { target, index } => {
                let t = match self.eval_expr(target).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                let i = match self.eval_expr(index).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                match (t, i) {
                    (Value::List(items), Value::Int(idx)) => {
                        let len = items.len();
                        let in_range = idx >= 0 && (idx as usize) < len;
                        if !in_range {
                            return Err(InterpError::new(
                                InterpErrorKind::IndexOutOfBounds { len, index: idx },
                                expr.span,
                            ));
                        }
                        Ok(ExprFlow::Value(items.get(idx as usize).expect("checked list index")))
                    }
                    (other, _) => Err(InterpError::new(
                        InterpErrorKind::TypeMismatch {
                            expected: "List".into(),
                            got: other.type_name(),
                        },
                        expr.span,
                    )),
                }
            }

            IrExprKind::BinOp { op, left, right } => {
                // Short-circuit `and` / `or`: evaluate the right operand
                // only when the left doesn't determine the result. This
                // matches the Cranelift lowering's merge-block pattern
                // and lets idioms like `true or (1 / 0 == 0)` return
                // `true` instead of raising.
                match op {
                    BinaryOp::And => {
                        let l = match self.eval_expr(left).await?.into_value() {
                            Ok(v) => v,
                            Err(v) => return Ok(ExprFlow::Propagate(v)),
                        };
                        let lb = require_bool(&l, left.span, "left operand of `and`")?;
                        if !lb {
                            return Ok(ExprFlow::Value(Value::Bool(false)));
                        }
                        let r = match self.eval_expr(right).await?.into_value() {
                            Ok(v) => v,
                            Err(v) => return Ok(ExprFlow::Propagate(v)),
                        };
                        let rb = require_bool(&r, right.span, "right operand of `and`")?;
                        return Ok(ExprFlow::Value(Value::Bool(rb)));
                    }
                    BinaryOp::Or => {
                        let l = match self.eval_expr(left).await?.into_value() {
                            Ok(v) => v,
                            Err(v) => return Ok(ExprFlow::Propagate(v)),
                        };
                        let lb = require_bool(&l, left.span, "left operand of `or`")?;
                        if lb {
                            return Ok(ExprFlow::Value(Value::Bool(true)));
                        }
                        let r = match self.eval_expr(right).await?.into_value() {
                            Ok(v) => v,
                            Err(v) => return Ok(ExprFlow::Propagate(v)),
                        };
                        let rb = require_bool(&r, right.span, "right operand of `or`")?;
                        return Ok(ExprFlow::Value(Value::Bool(rb)));
                    }
                    _ => {}
                }
                let l = match self.eval_expr(left).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                let r = match self.eval_expr(right).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(eval_binop(*op, l, r, expr.span)?))
            }

            IrExprKind::UnOp { op, operand } => {
                let v = match self.eval_expr(operand).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(eval_unop(*op, v, expr.span)?))
            }

            IrExprKind::List { items } => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    match self.eval_expr(it).await?.into_value() {
                        Ok(v) => out.push(v),
                        Err(v) => return Ok(ExprFlow::Propagate(v)),
                    }
                }
                Ok(ExprFlow::Value(Value::List(ListValue::new(out))))
            }

            IrExprKind::WeakNew { strong } => {
                let strong = match self.eval_expr(strong).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                let weak = strong.downgrade().ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::TypeMismatch {
                            expected: "String, Struct, or List".into(),
                            got: strong.type_name(),
                        },
                        expr.span,
                    )
                })?;
                Ok(ExprFlow::Value(Value::Weak(weak)))
            }

            IrExprKind::WeakUpgrade { weak } => {
                let weak = match self.eval_expr(weak).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                match weak {
                    Value::Weak(weak) => match weak.upgrade() {
                        Some(value) => Ok(ExprFlow::Value(Value::OptionSome(BoxedValue::new(value)))),
                        None => Ok(ExprFlow::Value(Value::OptionNone)),
                    },
                    other => Err(InterpError::new(
                        InterpErrorKind::TypeMismatch {
                            expected: "Weak".into(),
                            got: other.type_name(),
                        },
                        expr.span,
                    )),
                }
            }

            IrExprKind::ResultOk { inner } => {
                let v = match self.eval_expr(inner).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(Value::ResultOk(BoxedValue::new(v))))
            }

            IrExprKind::ResultErr { inner } => {
                let v = match self.eval_expr(inner).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(Value::ResultErr(BoxedValue::new(v))))
            }

            IrExprKind::OptionSome { inner } => {
                let v = match self.eval_expr(inner).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(Value::OptionSome(BoxedValue::new(v))))
            }

            IrExprKind::OptionNone => Ok(ExprFlow::Value(Value::OptionNone)),

            IrExprKind::TryPropagate { inner } => {
                let inner = match self.eval_expr(inner).await? {
                    ExprFlow::Value(v) => v,
                    ExprFlow::Propagate(v) => return Ok(ExprFlow::Propagate(v)),
                };
                match inner {
                    Value::ResultOk(v) => Ok(ExprFlow::Value(v.get())),
                    Value::ResultErr(v) => Ok(ExprFlow::Propagate(Value::ResultErr(v))),
                    Value::OptionSome(v) => Ok(ExprFlow::Value(v.get())),
                    Value::OptionNone => Ok(ExprFlow::Propagate(Value::OptionNone)),
                    other => Err(InterpError::new(
                        InterpErrorKind::TypeMismatch {
                            expected: "Result or Option".into(),
                            got: other.type_name(),
                        },
                        expr.span,
                    )),
                }
            }

            IrExprKind::TryRetry {
                body,
                attempts,
                backoff: _,
            } => {
                let total = (*attempts).max(1);
                let mut last_runtime_error: Option<InterpError> = None;
                let mut last_result_err: Option<Value> = None;
                let mut last_stream_start_err: Option<InterpError> = None;
                let mut last_stream_start_chunk: Option<StreamChunk> = None;
                let mut saw_option_retry = false;
                for _ in 0..total {
                    match self.eval_expr(body).await {
                        Ok(ExprFlow::Value(Value::Stream(stream))) => {
                            match stream.next_chunk().await {
                                Some(Ok(chunk)) if stream_start_is_retryable(&chunk.value) => {
                                    if matches!(chunk.value, Value::OptionNone) {
                                        saw_option_retry = true;
                                    } else {
                                        last_stream_start_chunk = Some(chunk);
                                    }
                                }
                                Some(Ok(chunk)) => {
                                    let combined = self.prepend_stream_chunk(chunk, stream);
                                    return Ok(ExprFlow::Value(combined));
                                }
                                Some(Err(err)) => {
                                    last_stream_start_err = Some(err);
                                }
                                None => return Ok(ExprFlow::Value(Value::Stream(stream))),
                            }
                        }
                        Ok(ExprFlow::Value(Value::ResultErr(err))) => {
                            last_result_err = Some(Value::ResultErr(err));
                        }
                        Ok(ExprFlow::Value(Value::OptionNone)) => {
                            saw_option_retry = true;
                        }
                        Ok(ExprFlow::Value(v)) => return Ok(ExprFlow::Value(v)),
                        Ok(ExprFlow::Propagate(v)) => return Ok(ExprFlow::Propagate(v)),
                        Err(err) => last_runtime_error = Some(err),
                    }
                }
                if let Some(v) = last_result_err {
                    Ok(ExprFlow::Value(v))
                } else if let Some(chunk) = last_stream_start_chunk {
                    Ok(ExprFlow::Value(self.singleton_stream(
                        chunk,
                        default_stream_backpressure(),
                    ).await?))
                } else if saw_option_retry {
                    Ok(ExprFlow::Value(Value::OptionNone))
                } else if let Some(err) = last_stream_start_err {
                    Ok(ExprFlow::Value(
                        self.singleton_stream_error(err, default_stream_backpressure()).await?,
                    ))
                } else if let Some(err) = last_runtime_error {
                    Err(err)
                } else {
                    Ok(ExprFlow::Value(Value::Nothing))
                }
            }
        }
    }

    /// Dispatch a call expression. Routes Tool / Prompt / Agent through
    /// the right runtime path; an `Unknown` kind is a hard error
    /// (typecheck should have caught it).
    async fn eval_call(
        &mut self,
        kind: &'ir IrCallKind,
        callee_name: &str,
        args: &'ir [IrExpr],
        result_ty: &Type,
        span: Span,
    ) -> Result<ExprFlow, InterpError> {
        // Evaluate args eagerly (left to right) before any external call.
        let mut arg_values = Vec::with_capacity(args.len());
        for a in args {
            match self.eval_expr(a).await?.into_value() {
                Ok(v) => arg_values.push(v),
                Err(v) => return Ok(ExprFlow::Propagate(v)),
            }
        }

        match kind {
            IrCallKind::Tool { def_id, .. } => {
                let tool = self.tools_by_id.get(def_id).copied().ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::DispatchFailed(format!(
                            "tool `{callee_name}` is missing from the IR"
                        )),
                        span,
                    )
                })?;

                // Runtime confidence gate: if the tool has
                // `trust: autonomous_if_confident(T)` in its declared
                // effects, check that composed input confidence >= T.
                // If below, raise an approval-required error — the
                // autonomous path is not safe for this specific call.
                if let Some(threshold) = tool.confidence_gate {
                    let actual = composed_confidence(&arg_values);
                    if actual < threshold {
                        return Err(InterpError::new(
                            InterpErrorKind::Runtime(
                                corvid_runtime::RuntimeError::ApprovalDenied {
                                    action: format!(
                                        "{callee_name}: confidence gate failed — composed input confidence {:.3} < required {:.3}. This tool declared `autonomous_if_confident({:.3})` but the runtime-observed confidence falls below threshold; human approval required.",
                                        actual, threshold, threshold,
                                    ),
                                },
                            ),
                            span,
                        ));
                    }
                }

                let json_args: Vec<serde_json::Value> =
                    arg_values.iter().map(value_to_json).collect();

                if self.should_yield_boundary() {
                    let action = self.maybe_yield(StepEvent::BeforeToolCall {
                        tool_name: callee_name.to_string(),
                        args: json_args.clone(),
                        span,
                        env: self.env_snapshot(),
                    }).await?;
                    if let StepAction::Override(val) = action {
                        return json_to_value(val, &tool.return_ty, &self.types_by_id)
                            .map(ExprFlow::Value)
                            .map_err(|e| InterpError::new(
                                InterpErrorKind::Marshal(format!("tool `{callee_name}` override: {e}")),
                                span,
                            ));
                    }
                }

                let start = std::time::Instant::now();
                let result = self
                    .runtime
                    .call_tool(callee_name, json_args)
                    .await
                    .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
                let elapsed_ms = start.elapsed().as_millis() as u64;

                if self.should_yield_boundary() {
                    let action = self.maybe_yield(StepEvent::AfterToolCall {
                        tool_name: callee_name.to_string(),
                        result: result.clone(),
                        elapsed_ms,
                        span,
                    }).await?;
                    if let StepAction::Override(val) = action {
                        return json_to_value(val, &tool.return_ty, &self.types_by_id)
                            .map(ExprFlow::Value)
                            .map_err(|e| InterpError::new(
                                InterpErrorKind::Marshal(format!("tool `{callee_name}` override: {e}")),
                                span,
                            ));
                    }
                }

                let value = json_to_value(result, &tool.return_ty, &self.types_by_id)
                    .map_err(|e| {
                    InterpError::new(
                        InterpErrorKind::Marshal(format!("tool `{callee_name}`: {e}")),
                        span,
                    )
                })?;

                // If the tool has a `retrieval` effect (data: grounded),
                // wrap the result in Grounded with a provenance chain.
                let is_grounded = tool.effect_names.iter().any(|e| e == "retrieval");
                if is_grounded {
                    let chain = crate::value::ProvenanceChain::with_retrieval(
                        callee_name,
                        corvid_runtime::now_ms(),
                    );
                    Ok(ExprFlow::Value(Value::Grounded(
                        crate::value::GroundedValue::new(value, chain),
                    )))
                } else {
                    Ok(ExprFlow::Value(value))
                }
            }
            IrCallKind::Prompt { def_id } => {
                let prompt = self.prompt_by_id(*def_id, callee_name, span)?;
                let result = self.dispatch_prompt(prompt, callee_name, &arg_values, span).await?;
                if !result.cost_charged && !matches!(&prompt.return_ty, Type::Stream(_)) {
                    self.charge_cost(result.cost, span)?;
                }
                self.finalize_prompt_result(prompt, result, span).await
            }
            IrCallKind::Agent { def_id } => {
                let agent = self.agents_by_id.get(def_id).copied().ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::DispatchFailed(format!(
                            "agent `{callee_name}` is missing from the IR"
                        )),
                        span,
                    )
                })?;

                if self.should_yield_boundary() {
                    let json_args: Vec<serde_json::Value> =
                        arg_values.iter().map(value_to_json).collect();
                    self.maybe_yield(StepEvent::BeforeAgentCall {
                        agent_name: callee_name.to_string(),
                        args: json_args,
                        span,
                    }).await?;
                }

                let mut sub = Interpreter::new(self.ir, self.runtime);
                // Propagate the step controller into sub-agent calls so
                // step-through continues across agent boundaries.
                if let Some(ref stepper) = self.stepper {
                    sub.stepper = Some(StepController::new(
                        Arc::clone(&stepper.hook_ref()),
                        stepper.mode,
                    ));
                }
                sub.bind_params(agent, arg_values)?;
                let result = sub.run_body(agent).await.map(ExprFlow::Value);

                if self.should_yield_boundary() {
                    let result_json = match &result {
                        Ok(ExprFlow::Value(v)) => value_to_json(v),
                        _ => serde_json::Value::Null,
                    };
                    self.maybe_yield(StepEvent::AfterAgentCall {
                        agent_name: callee_name.to_string(),
                        result: result_json,
                        span,
                    }).await?;
                }

                result
            }
            IrCallKind::StructConstructor { def_id } => {
                // Build a `Value::Struct` from the constructor args, in
                // field declaration order (mirrors the codegen-cl
                // lowering's store-at-offset pattern).
                let ir_type = self.types_by_id.get(def_id).copied().ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::DispatchFailed(format!(
                            "struct type `{callee_name}` is missing from the IR"
                        )),
                        span,
                    )
                })?;
                if arg_values.len() != ir_type.fields.len() {
                    return Err(InterpError::new(
                        InterpErrorKind::DispatchFailed(format!(
                            "struct constructor `{callee_name}` expects {} field(s), got {}",
                            ir_type.fields.len(),
                            arg_values.len(),
                        )),
                        span,
                    ));
                }
                let fields: Vec<(String, Value)> = ir_type
                    .fields
                    .iter()
                    .zip(arg_values.into_iter())
                    .map(|(f, v)| (f.name.clone(), v))
                    .collect();
                Ok(ExprFlow::Value(Value::Struct(crate::value::StructValue::new(
                    ir_type.id,
                    ir_type.name.clone(),
                    fields,
                ))))
            }
            IrCallKind::Unknown => {
                let _ = result_ty;
                Err(InterpError::new(
                    InterpErrorKind::DispatchFailed(format!(
                        "call to `{callee_name}` did not resolve to a tool, prompt, or agent"
                    )),
                    span,
                ))
            }
        }
    }
}

/// Render a prompt template by substituting `{paramname}` with the
/// JSON-serialized form of each argument. Unknown placeholders are left
/// alone — adapters that don't read `rendered` (like the mock) won't
/// notice.
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


