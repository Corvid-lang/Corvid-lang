//! Tree-walking interpreter, async edition.
//!
//! Asynchronous from the top because tool calls, prompt calls, and
//! approvals are async at the runtime boundary. The performance hit of
//! boxing recursive futures (via `async-recursion`) is the price for
//! keeping this tier behaviourally identical to the future Cranelift
//! backend, which will also be async-native. Behavioural parity is what
//! makes this interpreter useful as a correctness oracle.

use crate::conv::{json_to_value, value_to_json};
use crate::env::Env;
use crate::errors::{InterpError, InterpErrorKind};
use crate::value::{BoxedValue, ListValue, StructValue, Value};
use async_recursion::async_recursion;
use corvid_ast::{BinaryOp, Span, UnaryOp};
use corvid_ir::{
    IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrLiteral, IrPrompt, IrStmt,
    IrTool, IrType,
};
use corvid_resolve::{DefId, LocalId};
use corvid_runtime::{LlmRequest, Runtime, TraceEvent};
use crate::step::{self, StepAction, StepController, StepEvent, StepMode, StmtKind};
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

    async fn run_body(&mut self, agent: &'ir IrAgent) -> Result<Value, InterpError> {
        match self.eval_block(&agent.body).await? {
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
                let items = match iter_val {
                    Value::List(items) => items.iter_cloned(),
                    Value::String(s) => {
                        s.chars()
                            .map(|c| Value::String(Arc::from(c.to_string())))
                            .collect()
                    }
                    other => {
                        return Err(InterpError::new(
                            InterpErrorKind::TypeMismatch {
                                expected: "List or String".into(),
                                got: other.type_name(),
                            },
                            *span,
                        ))
                    }
                };
                for item in items {
                    self.env.bind(*var_local, item);
                    match self.eval_block(body).await? {
                        Flow::Normal | Flow::Continue => continue,
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Return(v) => return Ok(Flow::Return(v)),
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
                let mut saw_option_retry = false;
                for _ in 0..total {
                    match self.eval_expr(body).await {
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
                } else if saw_option_retry {
                    Ok(ExprFlow::Value(Value::OptionNone))
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
                let prompt = self.prompts_by_id.get(def_id).copied().ok_or_else(|| {
                    InterpError::new(
                        InterpErrorKind::DispatchFailed(format!(
                            "prompt `{callee_name}` is missing from the IR"
                        )),
                        span,
                    )
                })?;
                let json_args: Vec<serde_json::Value> =
                    arg_values.iter().map(value_to_json).collect();
                let rendered = render_prompt(prompt, &arg_values);

                if self.should_yield_boundary() {
                    let action = self.maybe_yield(StepEvent::BeforePromptCall {
                        prompt_name: callee_name.to_string(),
                        rendered: rendered.clone(),
                        model: None,
                        span,
                        env: self.env_snapshot(),
                    }).await?;
                    if let StepAction::Override(val) = action {
                        return json_to_value(val, &prompt.return_ty, &self.types_by_id)
                            .map(ExprFlow::Value)
                            .map_err(|e| InterpError::new(
                                InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                                span,
                            ));
                    }
                }

                let output_schema =
                    Some(crate::schema::schema_for(&prompt.return_ty, &self.types_by_id));
                let req = LlmRequest {
                    prompt: callee_name.to_string(),
                    model: String::new(),
                    rendered,
                    args: json_args,
                    output_schema,
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
                        return json_to_value(val, &prompt.return_ty, &self.types_by_id)
                            .map(ExprFlow::Value)
                            .map_err(|e| InterpError::new(
                                InterpErrorKind::Marshal(format!("prompt `{callee_name}` override: {e}")),
                                span,
                            ));
                    }
                }

                let value = json_to_value(resp.value, &prompt.return_ty, &self.types_by_id)
                    .map_err(|e| {
                        InterpError::new(
                            InterpErrorKind::Marshal(format!(
                                "prompt `{callee_name}`: {e}"
                            )),
                            span,
                        )
                    })?;

                // `cites ctx strictly` runtime verification: check that
                // the LLM response references content from the cited param.
                if let Some(param_idx) = prompt.cites_strictly_param {
                    if let Some(ctx_value) = arg_values.get(param_idx) {
                        let ctx_text = value_to_json(ctx_value).to_string();
                        let response_text = value_to_json(&value).to_string();
                        // Verify overlap: at least one substantial substring
                        // from the context appears in the response.
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

                // Provenance propagation: if any argument was Grounded,
                // the prompt's output inherits the provenance chain with
                // a PromptTransform entry added.
                let mut merged_chain = crate::value::ProvenanceChain::new();
                let mut has_grounded_input = false;
                for arg in &arg_values {
                    if let Value::Grounded(g) = arg {
                        merged_chain.merge(&g.provenance);
                        has_grounded_input = true;
                    }
                }
                if has_grounded_input {
                    merged_chain.add_prompt_transform(callee_name, corvid_runtime::now_ms());
                    Ok(ExprFlow::Value(Value::Grounded(
                        crate::value::GroundedValue::new(value, merged_chain),
                    )))
                } else {
                    Ok(ExprFlow::Value(value))
                }
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

fn eval_literal(lit: &IrLiteral) -> Value {
    match lit {
        IrLiteral::Int(n) => Value::Int(*n),
        IrLiteral::Float(f) => Value::Float(*f),
        IrLiteral::String(s) => Value::String(Arc::from(s.as_str())),
        IrLiteral::Bool(b) => Value::Bool(*b),
        IrLiteral::Nothing => Value::Nothing,
    }
}

fn eval_binop(op: BinaryOp, l: Value, r: Value, span: Span) -> Result<Value, InterpError> {
    use BinaryOp::*;
    match op {
        Add | Sub | Mul | Div | Mod => eval_arithmetic(op, l, r, span),
        Eq => Ok(Value::Bool(l == r)),
        NotEq => Ok(Value::Bool(l != r)),
        Lt | LtEq | Gt | GtEq => eval_ordering(op, l, r, span),
        // `and`/`or` are short-circuited inside `eval_expr` and never
        // reach this helper with both sides already evaluated.
        And | Or => unreachable!("and/or is short-circuited upstream"),
    }
}

fn eval_arithmetic(op: BinaryOp, l: Value, r: Value, span: Span) -> Result<Value, InterpError> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_arith(op, a, b, span)?)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_arith(op, a, b, span)?)),
        (Value::Int(a), Value::Float(b)) => {
            Ok(Value::Float(float_arith(op, a as f64, b, span)?))
        }
        (Value::Float(a), Value::Int(b)) => {
            Ok(Value::Float(float_arith(op, a, b as f64, span)?))
        }
        (Value::String(a), Value::String(b)) if matches!(op, BinaryOp::Add) => {
            let mut out = String::with_capacity(a.len() + b.len());
            out.push_str(&a);
            out.push_str(&b);
            Ok(Value::String(Arc::from(out)))
        }
        (a, b) => Err(InterpError::new(
            InterpErrorKind::TypeMismatch {
                expected: "Int or Float".into(),
                got: format!("{} and {}", a.type_name(), b.type_name()),
            },
            span,
        )),
    }
}

fn int_arith(op: BinaryOp, a: i64, b: i64, span: Span) -> Result<i64, InterpError> {
    use BinaryOp::*;
    match op {
        Add => a.checked_add(b).ok_or_else(|| overflow(span)),
        Sub => a.checked_sub(b).ok_or_else(|| overflow(span)),
        Mul => a.checked_mul(b).ok_or_else(|| overflow(span)),
        Div => {
            if b == 0 {
                Err(InterpError::new(
                    InterpErrorKind::Arithmetic("division by zero".into()),
                    span,
                ))
            } else {
                Ok(a.wrapping_div(b))
            }
        }
        Mod => {
            if b == 0 {
                Err(InterpError::new(
                    InterpErrorKind::Arithmetic("modulo by zero".into()),
                    span,
                ))
            } else {
                Ok(a.wrapping_rem(b))
            }
        }
        _ => unreachable!("non-arithmetic op routed here"),
    }
}

fn float_arith(op: BinaryOp, a: f64, b: f64, _span: Span) -> Result<f64, InterpError> {
    // Float arithmetic follows IEEE 754: `1.0 / 0.0 = +Inf`, `0.0 / 0.0
    // = NaN`, `Inf - Inf = NaN`. NaN propagation is the platform's
    // safety story for floats — telling callers "something went wrong
    // upstream" without aborting. Int arithmetic still traps on
    // overflow / div-by-zero because integers have no defined `Inf`.
    use BinaryOp::*;
    Ok(match op {
        Add => a + b,
        Sub => a - b,
        Mul => a * b,
        Div => a / b,
        Mod => a % b,
        _ => unreachable!("non-arithmetic op routed here"),
    })
}

fn eval_ordering(op: BinaryOp, l: Value, r: Value, span: Span) -> Result<Value, InterpError> {
    use BinaryOp::*;
    let ordering_result = |a: f64, b: f64| -> bool {
        match op {
            Lt => a < b,
            LtEq => a <= b,
            Gt => a > b,
            GtEq => a >= b,
            _ => unreachable!("non-ordering op routed here"),
        }
    };
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(match op {
            Lt => a < b,
            LtEq => a <= b,
            Gt => a > b,
            GtEq => a >= b,
            _ => unreachable!(),
        })),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(ordering_result(a, b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Bool(ordering_result(a as f64, b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(ordering_result(a, b as f64))),
        (Value::String(a), Value::String(b)) => Ok(Value::Bool(match op {
            Lt => a.as_ref() < b.as_ref(),
            LtEq => a.as_ref() <= b.as_ref(),
            Gt => a.as_ref() > b.as_ref(),
            GtEq => a.as_ref() >= b.as_ref(),
            _ => unreachable!(),
        })),
        (a, b) => Err(InterpError::new(
            InterpErrorKind::TypeMismatch {
                expected: "orderable (Int / Float / String)".into(),
                got: format!("{} and {}", a.type_name(), b.type_name()),
            },
            span,
        )),
    }
}

fn eval_unop(op: UnaryOp, v: Value, span: Span) -> Result<Value, InterpError> {
    match op {
        UnaryOp::Neg => match v {
            Value::Int(n) => n
                .checked_neg()
                .map(Value::Int)
                .ok_or_else(|| overflow(span)),
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Err(InterpError::new(
                InterpErrorKind::TypeMismatch {
                    expected: "Int or Float".into(),
                    got: other.type_name(),
                },
                span,
            )),
        },
        UnaryOp::Not => {
            let b = require_bool(&v, span, "operand of `not`")?;
            Ok(Value::Bool(!b))
        }
    }
}

fn require_bool(v: &Value, span: Span, context: &str) -> Result<bool, InterpError> {
    match v {
        Value::Bool(b) => Ok(*b),
        other => Err(InterpError::new(
            InterpErrorKind::TypeMismatch {
                expected: format!("Bool for {context}"),
                got: other.type_name(),
            },
            span,
        )),
    }
}

fn overflow(span: Span) -> InterpError {
    InterpError::new(
        InterpErrorKind::Arithmetic("integer overflow".into()),
        span,
    )
}

/// Citation verification for `cites ctx strictly`. Checks that the
/// LLM response contains at least one substantial substring from the
/// context. "Substantial" means a contiguous run of words (≥ 4 words)
/// from the context appears verbatim in the response.
fn citation_verified(context: &str, response: &str) -> bool {
    let ctx_lower = context.to_lowercase();
    let resp_lower = response.to_lowercase();

    // Extract word sequences from context and check for matches.
    let ctx_words: Vec<&str> = ctx_lower.split_whitespace().collect();
    if ctx_words.len() < 4 {
        // Short context: check if the whole thing appears.
        return resp_lower.contains(&ctx_lower);
    }

    // Sliding window: check if any 4-word sequence from context
    // appears in the response.
    let window_size = 4;
    for window in ctx_words.windows(window_size) {
        let phrase = window.join(" ");
        if resp_lower.contains(&phrase) {
            return true;
        }
    }

    false
}

// `Type` import: needed by `eval_call`'s signature. Imported here at the
// bottom so the file's structure stays close to the original.
use corvid_types::Type;

// Suppress dead-field warnings on the few fields the interpreter holds
// but doesn't yet read directly (the IR is read indirectly via the
// per-kind id maps).
#[allow(dead_code)]
fn _force_use(i: &Interpreter<'_>) {
    let _ = &i.ir;
    let _ = &i.types_by_id;
}
