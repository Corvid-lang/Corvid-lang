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
use crate::value::{StructValue, Value};
use async_recursion::async_recursion;
use corvid_ast::{BinaryOp, Span, UnaryOp};
use corvid_ir::{
    IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrLiteral, IrPrompt, IrStmt,
    IrTool, IrType,
};
use corvid_resolve::{DefId, LocalId};
use corvid_runtime::{LlmRequest, Runtime, TraceEvent};
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
    });

    let mut interp = Interpreter::new(ir, runtime);
    let bind_result = interp.bind_params(agent, args);
    let outcome = match bind_result {
        Ok(()) => interp.run_body(agent).await,
        Err(e) => Err(e),
    };

    runtime.tracer().emit(TraceEvent::RunCompleted {
        ts_ms: corvid_runtime::now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        ok: outcome.is_ok(),
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
    Value::Struct(Arc::new(StructValue {
        type_id,
        type_name: type_name.to_string(),
        fields: fields.into_iter().collect(),
    }))
}

/// Control-flow outcome of evaluating a statement or block.
#[derive(Debug, Clone)]
enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

struct Interpreter<'ir> {
    ir: &'ir IrFile,
    env: Env,
    types_by_id: HashMap<DefId, &'ir IrType>,
    tools_by_id: HashMap<DefId, &'ir IrTool>,
    prompts_by_id: HashMap<DefId, &'ir IrPrompt>,
    agents_by_id: HashMap<DefId, &'ir IrAgent>,
    runtime: &'ir Runtime,
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
            self.env.bind(p.local_id, v);
        }
        Ok(())
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
            IrStmt::Let { local_id, value, .. } => {
                let v = self.eval_expr(value).await?;
                self.env.bind(*local_id, v);
                Ok(Flow::Normal)
            }
            IrStmt::Return { value, .. } => {
                let v = match value {
                    Some(e) => self.eval_expr(e).await?,
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
                let c = self.eval_expr(cond).await?;
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
                let iter_val = self.eval_expr(iter).await?;
                let items = match iter_val {
                    Value::List(items) => items,
                    Value::String(s) => {
                        let chars: Vec<Value> = s
                            .chars()
                            .map(|c| Value::String(Arc::from(c.to_string())))
                            .collect();
                        Arc::new(chars)
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
                for item in items.iter() {
                    self.env.bind(*var_local, item.clone());
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
                    let v = self.eval_expr(a).await?;
                    json_args.push(value_to_json(&v));
                }
                self.runtime
                    .approval_gate(label, json_args)
                    .await
                    .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), *span))?;
                Ok(Flow::Normal)
            }
            IrStmt::Expr { expr, .. } => {
                let _ = self.eval_expr(expr).await?;
                Ok(Flow::Normal)
            }
            IrStmt::Break { .. } => Ok(Flow::Break),
            IrStmt::Continue { .. } => Ok(Flow::Continue),
            IrStmt::Pass { .. } => Ok(Flow::Normal),
        }
    }

    #[async_recursion]
    async fn eval_expr(&mut self, expr: &'ir IrExpr) -> Result<Value, InterpError> {
        match &expr.kind {
            IrExprKind::Literal(lit) => Ok(eval_literal(lit)),

            IrExprKind::Local { local_id, .. } => {
                self.env.lookup(*local_id).ok_or_else(|| {
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
                let t = self.eval_expr(target).await?;
                match t {
                    Value::Struct(s) => s.fields.get(field).cloned().ok_or_else(|| {
                        InterpError::new(
                            InterpErrorKind::UnknownField {
                                struct_name: s.type_name.clone(),
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
                let t = self.eval_expr(target).await?;
                let i = self.eval_expr(index).await?;
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
                        Ok(items[idx as usize].clone())
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
                        let l = self.eval_expr(left).await?;
                        let lb = require_bool(&l, left.span, "left operand of `and`")?;
                        if !lb {
                            return Ok(Value::Bool(false));
                        }
                        let r = self.eval_expr(right).await?;
                        let rb = require_bool(&r, right.span, "right operand of `and`")?;
                        return Ok(Value::Bool(rb));
                    }
                    BinaryOp::Or => {
                        let l = self.eval_expr(left).await?;
                        let lb = require_bool(&l, left.span, "left operand of `or`")?;
                        if lb {
                            return Ok(Value::Bool(true));
                        }
                        let r = self.eval_expr(right).await?;
                        let rb = require_bool(&r, right.span, "right operand of `or`")?;
                        return Ok(Value::Bool(rb));
                    }
                    _ => {}
                }
                let l = self.eval_expr(left).await?;
                let r = self.eval_expr(right).await?;
                eval_binop(*op, l, r, expr.span)
            }

            IrExprKind::UnOp { op, operand } => {
                let v = self.eval_expr(operand).await?;
                eval_unop(*op, v, expr.span)
            }

            IrExprKind::List { items } => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval_expr(it).await?);
                }
                Ok(Value::List(Arc::new(out)))
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
    ) -> Result<Value, InterpError> {
        // Evaluate args eagerly (left to right) before any external call.
        let mut arg_values = Vec::with_capacity(args.len());
        for a in args {
            arg_values.push(self.eval_expr(a).await?);
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
                let result = self
                    .runtime
                    .call_tool(callee_name, json_args)
                    .await
                    .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
                json_to_value(result, &tool.return_ty, &self.types_by_id).map_err(|e| {
                    InterpError::new(
                        InterpErrorKind::Marshal(format!("tool `{callee_name}`: {e}")),
                        span,
                    )
                })
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
                let output_schema =
                    Some(crate::schema::schema_for(&prompt.return_ty, &self.types_by_id));
                let req = LlmRequest {
                    prompt: callee_name.to_string(),
                    model: String::new(),
                    rendered,
                    args: json_args,
                    output_schema,
                };
                let resp = self
                    .runtime
                    .call_llm(req)
                    .await
                    .map_err(|e| InterpError::new(InterpErrorKind::Runtime(e), span))?;
                json_to_value(resp.value, &prompt.return_ty, &self.types_by_id).map_err(
                    |e| {
                        InterpError::new(
                            InterpErrorKind::Marshal(format!(
                                "prompt `{callee_name}`: {e}"
                            )),
                            span,
                        )
                    },
                )
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
                let mut sub = Interpreter::new(self.ir, self.runtime);
                sub.bind_params(agent, arg_values)?;
                sub.run_body(agent).await
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

fn float_arith(op: BinaryOp, a: f64, b: f64, span: Span) -> Result<f64, InterpError> {
    use BinaryOp::*;
    match op {
        Add => Ok(a + b),
        Sub => Ok(a - b),
        Mul => Ok(a * b),
        Div => {
            if b == 0.0 {
                Err(InterpError::new(
                    InterpErrorKind::Arithmetic("division by zero".into()),
                    span,
                ))
            } else {
                Ok(a / b)
            }
        }
        Mod => {
            if b == 0.0 {
                Err(InterpError::new(
                    InterpErrorKind::Arithmetic("modulo by zero".into()),
                    span,
                ))
            } else {
                Ok(a % b)
            }
        }
        _ => unreachable!("non-arithmetic op routed here"),
    }
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
