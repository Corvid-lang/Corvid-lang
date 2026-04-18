use super::{Flow, Interpreter};
use crate::errors::{InterpError, InterpErrorKind};
use crate::step::{StepAction, StepEvent, StmtKind};
use crate::value::{StreamChunk, StreamValue, Value};
use crate::value_to_json;
use async_recursion::async_recursion;
use corvid_ast::{BackpressurePolicy, Span};
use corvid_ir::{IrAgent, IrBlock, IrExpr, IrExprKind, IrStmt};
use corvid_types::Type;
use std::sync::Arc;

impl<'ir> Interpreter<'ir> {
    pub(super) async fn run_body(&mut self, agent: &'ir IrAgent) -> Result<Value, InterpError> {
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
                InterpErrorKind::Other("loop control flow escaped its enclosing loop".into()),
                agent.span,
            )),
        }
    }

    async fn spawn_stream_agent(&mut self, agent: &'ir IrAgent) -> Result<Value, InterpError> {
        let (sender, stream) =
            StreamValue::channel(super::effect_compose::default_stream_backpressure());
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
                        let _ = sender
                            .send(Err(InterpError::new(
                                InterpErrorKind::Other(
                                    "loop control flow escaped its enclosing loop".into(),
                                ),
                                agent.span,
                            )))
                            .await;
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

    pub(super) async fn singleton_stream(
        &self,
        chunk: StreamChunk,
        backpressure: BackpressurePolicy,
    ) -> Result<Value, InterpError> {
        let (sender, stream) = StreamValue::channel(backpressure);
        let _ = sender.send_chunk(Ok(chunk)).await;
        Ok(Value::Stream(stream))
    }

    pub(super) async fn singleton_stream_error(
        &self,
        err: InterpError,
        backpressure: BackpressurePolicy,
    ) -> Result<Value, InterpError> {
        let (sender, stream) = StreamValue::channel(backpressure);
        let _ = sender.send_chunk(Err(err)).await;
        Ok(Value::Stream(stream))
    }

    pub(super) fn prepend_stream_chunk(&self, first: StreamChunk, stream: StreamValue) -> Value {
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

    pub(super) fn charge_cost(&mut self, cost: f64, span: Span) -> Result<(), InterpError> {
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
    pub(super) async fn eval_block(
        &mut self,
        block: &'ir IrBlock,
    ) -> Result<Flow, InterpError> {
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
            IrStmt::Let {
                local_id,
                name,
                value,
                ..
            } => {
                if self.should_yield_statement() {
                    self.maybe_yield(StepEvent::BeforeStatement {
                        kind: StmtKind::Let { name: name.clone() },
                        span: value.span,
                        env: self.env_snapshot(),
                    })
                    .await?;
                }
                let v = match self.eval_expr(value).await?.into_value() {
                    Ok(v) => v,
                    Err(v) => return Ok(Flow::Return(v)),
                };
                self.env.bind(*local_id, v);
                self.local_names.insert(*local_id, name.clone());
                if let IrExprKind::Local {
                    local_id: source_local,
                    ..
                } = &value.kind
                {
                    if let Some(chunk) = self.stream_locals.get(source_local).cloned() {
                        self.stream_locals.insert(
                            *local_id,
                            StreamChunk {
                                value: self.env.lookup(*local_id).unwrap_or(Value::Nothing),
                                ..chunk
                            },
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
                        ));
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
                        ));
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
                    let action = self
                        .maybe_yield(StepEvent::BeforeApproval {
                            label: label.clone(),
                            args: json_args.clone(),
                            span: *span,
                            env: self.env_snapshot(),
                        })
                        .await?;

                    match action {
                        StepAction::Approve => {
                            if self.should_yield_boundary() {
                                self.maybe_yield(StepEvent::AfterApproval {
                                    label: label.clone(),
                                    approved: true,
                                    span: *span,
                                })
                                .await?;
                            }
                            return Ok(Flow::Normal);
                        }
                        StepAction::Deny => {
                            if self.should_yield_boundary() {
                                self.maybe_yield(StepEvent::AfterApproval {
                                    label: label.clone(),
                                    approved: false,
                                    span: *span,
                                })
                                .await?;
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

                let result = self.runtime.approval_gate(label, json_args).await;
                let approved = result.is_ok();

                if self.should_yield_boundary() {
                    self.maybe_yield(StepEvent::AfterApproval {
                        label: label.clone(),
                        approved,
                        span: *span,
                    })
                    .await?;
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
            IrStmt::Dup { .. } | IrStmt::Drop { .. } => Ok(Flow::Normal),
        }
    }
}
