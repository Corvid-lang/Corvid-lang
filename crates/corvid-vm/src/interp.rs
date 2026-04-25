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
#[path = "interp/prompt.rs"]
mod prompt;
#[path = "interp/replay.rs"]
mod replay;
#[path = "interp/stmt.rs"]
mod stmt;

use crate::conv::{json_to_value, value_to_json};
use crate::env::Env;
use crate::errors::{InterpError, InterpErrorKind};
use crate::step::{self, ConfidenceGateStep, StepAction, StepController, StepEvent, StepMode};
use crate::value::{
    value_confidence, BoxedValue, ListValue, StreamChunk, StreamSender, StructValue, Value,
};
use self::expr::{eval_binop, eval_literal, eval_unop, require_bool};
use effect_compose::{
    composed_confidence, default_stream_backpressure, stream_start_is_retryable,
};
use async_recursion::async_recursion;
use corvid_ast::{BinaryOp, Span};
use corvid_ir::{IrAgent, IrCallKind, IrExpr, IrExprKind, IrFile, IrPrompt, IrTool, IrType};
use corvid_resolve::{DefId, LocalId};
use corvid_runtime::{Runtime, RuntimeError, TraceEvent};
use corvid_types::Type;
use std::collections::HashMap;
use std::sync::Arc;

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

    let json_args: Vec<serde_json::Value> = args.iter().map(value_to_json).collect();
    runtime
        .prepare_run(agent_name, &json_args)
        .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), Span::new(0, 0)))?;

    runtime.tracer().emit(TraceEvent::RunStarted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        agent: agent_name.to_string(),
        args: json_args,
    });

    let mut interp = Interpreter::new(ir, runtime);
    let bind_result = interp.bind_params(agent, args);
    let outcome = match bind_result {
        Ok(()) => interp.run_body(agent).await.map(|value| (value, interp.env.clone())),
        Err(e) => Err(e),
    };

    let result_json = outcome
        .as_ref()
        .ok()
        .map(|(value, _env)| value_to_json(value));
    let error_text = outcome.as_ref().err().map(|error| error.to_string());
    if should_validate_run_completion(&outcome) {
        runtime
            .complete_run(
                outcome.is_ok(),
                result_json.as_ref(),
                error_text.as_deref(),
            )
            .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), Span::new(0, 0)))?;
    }
    runtime.tracer().emit(TraceEvent::RunCompleted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        ok: outcome.is_ok(),
        result: result_json,
        error: error_text,
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

    let json_args: Vec<serde_json::Value> = args.iter().map(value_to_json).collect();
    runtime
        .prepare_run(agent_name, &json_args)
        .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), Span::new(0, 0)))?;

    runtime.tracer().emit(TraceEvent::RunStarted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        agent: agent_name.to_string(),
        args: json_args,
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
        result_confidence: outcome.as_ref().ok().map(|(v, _)| value_confidence(v)),
        error: outcome.as_ref().err().map(|e| e.to_string()),
    }).await;

    let result_json = outcome.as_ref().ok().map(|(value, _)| value_to_json(value));
    let error_text = outcome.as_ref().err().map(|error| error.to_string());
    if should_validate_run_completion(&outcome) {
        runtime
            .complete_run(
                outcome.is_ok(),
                result_json.as_ref(),
                error_text.as_deref(),
            )
            .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), Span::new(0, 0)))?;
    }
    runtime.tracer().emit(TraceEvent::RunCompleted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        ok: outcome.is_ok(),
        result: result_json,
        error: error_text,
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

fn should_validate_run_completion(outcome: &Result<(Value, Env), InterpError>) -> bool {
    !matches!(
        outcome,
        Err(InterpError {
            kind: InterpErrorKind::Runtime(
                RuntimeError::ReplayDivergence(_)
                    | RuntimeError::ReplayTraceLoad { .. }
                    | RuntimeError::CrossTierReplayUnsupported { .. }
            ),
            ..
        })
    )
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
                    Value::Partial(p) => p.get_field(field).map(ExprFlow::Value).ok_or_else(|| {
                        InterpError::new(
                            InterpErrorKind::UnknownField {
                                struct_name: p.type_name().to_string(),
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

            IrExprKind::UnwrapGrounded { value } => {
                let value = match self.eval_expr(value).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                match value {
                    Value::Grounded(grounded) => Ok(ExprFlow::Value(grounded.inner.get())),
                    other => Err(InterpError::new(
                        InterpErrorKind::TypeMismatch {
                            expected: "Grounded<T>".into(),
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
                Ok(ExprFlow::Value(eval_binop(*op, l, r, expr.span, false)?))
            }

            IrExprKind::WrappingBinOp { op, left, right } => {
                let l = match self.eval_expr(left).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                let r = match self.eval_expr(right).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(eval_binop(*op, l, r, expr.span, true)?))
            }

            IrExprKind::UnOp { op, operand } => {
                let v = match self.eval_expr(operand).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(eval_unop(*op, v, expr.span, false)?))
            }

            IrExprKind::WrappingUnOp { op, operand } => {
                let v = match self.eval_expr(operand).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(ExprFlow::Propagate(v)),
                };
                Ok(ExprFlow::Value(eval_unop(*op, v, expr.span, true)?))
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

            IrExprKind::Replay {
                trace,
                arms,
                else_body,
            } => self.eval_replay_expr(trace, arms, else_body, expr.span).await,
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

                let json_args: Vec<serde_json::Value> =
                    arg_values.iter().map(value_to_json).collect();

                // Runtime confidence gate: if the tool has
                // `trust: autonomous_if_confident(T)` in its declared
                // effects, check that composed input confidence >= T.
                // If below, activate the same approval path used by
                // explicit `approve` statements before dispatching the
                // tool.
                let input_confidence = composed_confidence(&arg_values);
                let confidence_gate = tool.confidence_gate.map(|threshold| ConfidenceGateStep {
                    threshold,
                    actual: input_confidence,
                    triggered: input_confidence < threshold,
                });
                if let Some(gate) = confidence_gate {
                    if gate.triggered {
                        let label = format!("ConfidenceGate:{callee_name}");
                        if self.should_yield_boundary() {
                            let action = self
                                .maybe_yield(StepEvent::BeforeApproval {
                                    label: label.clone(),
                                    args: json_args.clone(),
                                    confidence_gate: Some(gate),
                                    span,
                                    env: self.env_snapshot(),
                                })
                                .await?;
                            match action {
                                StepAction::Approve => {
                                    self.maybe_yield(StepEvent::AfterApproval {
                                        label,
                                        approved: true,
                                        span,
                                    })
                                    .await?;
                                }
                                StepAction::Deny => {
                                    self.maybe_yield(StepEvent::AfterApproval {
                                        label: label.clone(),
                                        approved: false,
                                        span,
                                    })
                                    .await?;
                                    return Err(InterpError::new(
                                        InterpErrorKind::Runtime(
                                            corvid_runtime::RuntimeError::ApprovalDenied {
                                                action: label,
                                            },
                                        ),
                                        span,
                                    ));
                                }
                                _ => {
                                    let result =
                                        self.runtime.approval_gate(&label, json_args.clone()).await;
                                    let approved = result.is_ok();
                                    self.maybe_yield(StepEvent::AfterApproval {
                                        label,
                                        approved,
                                        span,
                                    })
                                    .await?;
                                    result.map_err(|e| {
                                        InterpError::new(InterpErrorKind::Runtime(e), span)
                                    })?;
                                }
                            }
                        } else {
                            self.runtime
                                .approval_gate(&label, json_args.clone())
                                .await
                                .map_err(|e| {
                                    InterpError::new(InterpErrorKind::Runtime(e), span)
                                })?;
                        }
                    }
                }
                let is_grounded = tool.effect_names.iter().any(|e| e == "retrieval");
                let result_decode_ty = match (&tool.return_ty, is_grounded) {
                    (Type::Grounded(inner), true) => inner.as_ref(),
                    _ => &tool.return_ty,
                };

                if self.should_yield_boundary() {
                    let action = self.maybe_yield(StepEvent::BeforeToolCall {
                        tool_name: callee_name.to_string(),
                        args: json_args.clone(),
                        input_confidence,
                        confidence_gate,
                        span,
                        env: self.env_snapshot(),
                    }).await?;
                    if let StepAction::Override(val) = action {
                        let value = json_to_value(val, result_decode_ty, &self.types_by_id)
                            .map_err(|e| InterpError::new(
                                InterpErrorKind::Marshal(format!("tool `{callee_name}` override: {e}")),
                                span,
                            ))?;
                        return Ok(ExprFlow::Value(maybe_ground_tool_result(
                            tool,
                            callee_name,
                            value,
                        )));
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
                        result_confidence: 1.0,
                        elapsed_ms,
                        span,
                    }).await?;
                    if let StepAction::Override(val) = action {
                        let value = json_to_value(val, result_decode_ty, &self.types_by_id)
                            .map_err(|e| InterpError::new(
                                InterpErrorKind::Marshal(format!("tool `{callee_name}` override: {e}")),
                                span,
                            ))?;
                        return Ok(ExprFlow::Value(maybe_ground_tool_result(
                            tool,
                            callee_name,
                            value,
                        )));
                    }
                }

                let value = json_to_value(result, result_decode_ty, &self.types_by_id)
                    .map_err(|e| {
                    InterpError::new(
                        InterpErrorKind::Marshal(format!("tool `{callee_name}`: {e}")),
                        span,
                    )
                })?;

                // If the tool has a `retrieval` effect (data: grounded),
                // wrap the result in Grounded with a provenance chain.
                Ok(ExprFlow::Value(maybe_ground_tool_result(
                    tool,
                    callee_name,
                    value,
                )))
            }
            IrCallKind::Prompt { def_id } => {
                self.dispatch_prompt_expr(*def_id, callee_name, &arg_values, span)
                    .await
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
                        input_confidence: composed_confidence(&arg_values),
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
                        result_confidence: result
                            .as_ref()
                            .ok()
                            .and_then(|flow| match flow {
                                ExprFlow::Value(value) => Some(value_confidence(value)),
                                ExprFlow::Propagate(value) => Some(value_confidence(value)),
                            })
                            .unwrap_or(1.0),
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

fn maybe_ground_tool_result(tool: &IrTool, callee_name: &str, value: Value) -> Value {
    if !tool.effect_names.iter().any(|e| e == "retrieval") {
        return value;
    }

    let chain = crate::ProvenanceChain::with_retrieval(callee_name, corvid_runtime::now_ms());
    Value::Grounded(crate::value::GroundedValue::new(value, chain))
}
