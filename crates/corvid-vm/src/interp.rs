//! Tree-walking interpreter.
//!
//! Consumes an `IrFile` and executes agent bodies. This slice covers pure
//! computation only — literals, locals, arithmetic, comparisons, control
//! flow, struct field access, and list operations.
//!
//! Tool calls, prompt calls, agent calls, and `approve` statements produce
//! `InterpErrorKind::NotImplemented` for now; those are added in the next
//! sub-phase alongside the native runtime (`corvid-runtime`).

use crate::env::Env;
use crate::errors::{InterpError, InterpErrorKind};
use crate::value::{StructValue, Value};
use corvid_ast::{BinaryOp, Span, UnaryOp};
use corvid_ir::{
    IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrLiteral, IrStmt, IrType,
};
use corvid_resolve::{DefId, LocalId};
use std::collections::HashMap;
use std::sync::Arc;

/// Public interpreter entry point: run `agent_name` with `args`.
///
/// Returns the agent's `Return`-ed value, or the span-carrying error that
/// aborted execution.
pub fn run_agent(
    ir: &IrFile,
    agent_name: &str,
    args: Vec<Value>,
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

    let mut interp = Interpreter::new(ir);
    interp.bind_params(agent, args)?;
    interp.run_body(agent)
}

/// Control-flow outcome of evaluating a statement or block.
#[derive(Debug, Clone)]
enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

/// Type-index, used by struct constructors during lowering.
struct TypeIndex {
    by_name: HashMap<String, DefId>,
}

impl TypeIndex {
    fn new(ir: &IrFile) -> Self {
        let mut by_name = HashMap::new();
        for t in &ir.types {
            by_name.insert(t.name.clone(), t.id);
        }
        Self { by_name }
    }
}

struct Interpreter<'ir> {
    #[allow(dead_code)]
    ir: &'ir IrFile,
    env: Env,
    #[allow(dead_code)]
    types: TypeIndex,
    #[allow(dead_code)]
    types_by_id: HashMap<DefId, &'ir IrType>,
}

impl<'ir> Interpreter<'ir> {
    fn new(ir: &'ir IrFile) -> Self {
        let types = TypeIndex::new(ir);
        let types_by_id: HashMap<DefId, &IrType> =
            ir.types.iter().map(|t| (t.id, t)).collect();
        Self {
            ir,
            env: Env::new(),
            types,
            types_by_id,
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

    fn run_body(&mut self, agent: &'ir IrAgent) -> Result<Value, InterpError> {
        match self.eval_block(&agent.body)? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => {
                // The return statement is syntactically optional in v0.1 only
                // for agents returning `Nothing`. Any other shape is the type
                // checker's responsibility; the interpreter surfaces the gap.
                Ok(Value::Nothing)
            }
            Flow::Break | Flow::Continue => Err(InterpError::new(
                InterpErrorKind::Other(format!(
                    "`{}` escaped its enclosing loop",
                    match self.eval_block(&agent.body)? {
                        Flow::Break => "break",
                        Flow::Continue => "continue",
                        _ => "control",
                    }
                )),
                agent.span,
            )),
        }
    }

    fn eval_block(&mut self, block: &'ir IrBlock) -> Result<Flow, InterpError> {
        for stmt in &block.stmts {
            match self.eval_stmt(stmt)? {
                Flow::Normal => continue,
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn eval_stmt(&mut self, stmt: &'ir IrStmt) -> Result<Flow, InterpError> {
        match stmt {
            IrStmt::Let { local_id, value, .. } => {
                let v = self.eval_expr(value)?;
                self.env.bind(*local_id, v);
                Ok(Flow::Normal)
            }
            IrStmt::Return { value, .. } => {
                let v = match value {
                    Some(e) => self.eval_expr(e)?,
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
                let c = self.eval_expr(cond)?;
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
                    self.eval_block(then_block)
                } else if let Some(eb) = else_block {
                    self.eval_block(eb)
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
                let iter_val = self.eval_expr(iter)?;
                let items = match iter_val {
                    Value::List(items) => items,
                    Value::String(s) => {
                        // Strings iterate as single-char String values.
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
                    match self.eval_block(body)? {
                        Flow::Normal | Flow::Continue => continue,
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                    }
                }
                Ok(Flow::Normal)
            }
            IrStmt::Approve { span, .. } => Err(InterpError::new(
                InterpErrorKind::NotImplemented("approve statements".into()),
                *span,
            )),
            IrStmt::Expr { expr, .. } => {
                let _ = self.eval_expr(expr)?;
                Ok(Flow::Normal)
            }
            IrStmt::Break { .. } => Ok(Flow::Break),
            IrStmt::Continue { .. } => Ok(Flow::Continue),
            IrStmt::Pass { .. } => Ok(Flow::Normal),
        }
    }

    fn eval_expr(&mut self, expr: &'ir IrExpr) -> Result<Value, InterpError> {
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

            IrExprKind::Call { kind, callee_name, .. } => {
                let what = match kind {
                    IrCallKind::Tool { .. } => "tool calls",
                    IrCallKind::Prompt { .. } => "prompt calls",
                    IrCallKind::Agent { .. } => "agent-to-agent calls",
                    IrCallKind::Unknown => "unresolved calls",
                };
                Err(InterpError::new(
                    InterpErrorKind::NotImplemented(format!(
                        "{what} (at `{callee_name}`)"
                    )),
                    expr.span,
                ))
            }

            IrExprKind::FieldAccess { target, field } => {
                let t = self.eval_expr(target)?;
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
                let t = self.eval_expr(target)?;
                let i = self.eval_expr(index)?;
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
                let l = self.eval_expr(left)?;
                let r = self.eval_expr(right)?;
                eval_binop(*op, l, r, expr.span)
            }

            IrExprKind::UnOp { op, operand } => {
                let v = self.eval_expr(operand)?;
                eval_unop(*op, v, expr.span)
            }

            IrExprKind::List { items } => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval_expr(it)?);
                }
                Ok(Value::List(Arc::new(out)))
            }
        }
    }
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
        And => {
            let lb = require_bool(&l, span, "left operand of `and`")?;
            let rb = require_bool(&r, span, "right operand of `and`")?;
            Ok(Value::Bool(lb && rb))
        }
        Or => {
            let lb = require_bool(&l, span, "left operand of `or`")?;
            let rb = require_bool(&r, span, "right operand of `or`")?;
            Ok(Value::Bool(lb || rb))
        }
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

// Suppress the unused-field warning for fields we'll start using in the
// next slice (tool calls, prompt calls, approve). The `_` prefixes on
// arguments already cover most, but the struct fields need this pragma.
#[allow(dead_code)]
fn _force_use(i: &Interpreter<'_>) {
    let _ = &i.ir;
    let _ = &i.types;
    let _ = &i.types_by_id;
}

/// Build a struct `Value` from field name → value pairs. Used by the
/// runtime when converting native tool/prompt results into Corvid values
/// during the next slice of Phase 11.
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

/// Expose the interpreter creator so the runtime can bind locals and run
/// isolated snippets in future slices.
pub fn bind_and_run_agent(
    ir: &IrFile,
    agent_name: &str,
    params_with_values: Vec<(LocalId, Value)>,
    fallback_args: Vec<Value>,
) -> Result<Value, InterpError> {
    // Pre-bind known locals (used when the runtime wants to inject
    // pre-constructed struct params in tests).
    if params_with_values.is_empty() {
        return run_agent(ir, agent_name, fallback_args);
    }
    let agent = ir.agents.iter().find(|a| a.name == agent_name).ok_or_else(|| {
        InterpError::new(
            InterpErrorKind::DispatchFailed(format!("no agent named `{agent_name}`")),
            Span::new(0, 0),
        )
    })?;
    let mut interp = Interpreter::new(ir);
    for (id, v) in params_with_values {
        interp.env.bind(id, v);
    }
    interp.run_body(agent)
}
