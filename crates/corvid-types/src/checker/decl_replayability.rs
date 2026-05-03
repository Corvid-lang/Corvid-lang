//! Replayability and deterministic declaration checking.

use super::Checker;
use crate::determinism::{classify_call_target, NondeterminismSource};
use crate::errors::{TypeError, TypeErrorKind};
use corvid_ast::{AgentAttribute, Block, Expr, Span, Stmt};
use corvid_resolve::{Binding, BuiltIn, DeclKind};

impl<'a> Checker<'a> {
    /// Walk `body` looking for calls to functions the determinism
    /// catalog flags as nondeterministic. Emits one
    /// `NonReplayableCall` error per offending call site. Safe to
    /// call on any body Ã¢â‚¬â€ a catalog-empty walk is a no-op.
    pub(super) fn check_replayable_body(&mut self, agent_name: &str, body: &Block) {
        let mut violations = Vec::new();
        collect_replayability_violations_in_block(body, &mut violations);
        for violation in violations {
            self.errors.push(TypeError::with_guarantee(
                TypeErrorKind::NonReplayableCall {
                    agent: agent_name.to_string(),
                    call: violation.call_name,
                    source_label: violation.source.label().to_string(),
                },
                violation.span,
                "replay.deterministic_pure_path",
            ));
        }
    }

    /// Walk `body` enforcing the `@deterministic` contract: no
    /// LLM prompt calls, no tool calls, no approve statements,
    /// no catalog-registered nondeterminism, and every called
    /// agent must itself be `@deterministic`. Needs resolver
    /// access (to classify call targets by `DeclKind`) so this
    /// lives on `Checker` rather than as a free helper like the
    /// replayability walk.
    pub(super) fn check_deterministic_body(&mut self, agent_name: &str, body: &Block) {
        self.walk_deterministic_block(agent_name, body);
    }

    fn walk_deterministic_block(&mut self, agent: &str, block: &Block) {
        for stmt in &block.stmts {
            self.walk_deterministic_stmt(agent, stmt);
        }
    }

    fn walk_deterministic_stmt(&mut self, agent: &str, stmt: &Stmt) {
        match stmt {
            Stmt::Let { value, .. } => self.walk_deterministic_expr(agent, value),
            Stmt::Return { value, .. } => {
                if let Some(expr) = value {
                    self.walk_deterministic_expr(agent, expr);
                }
            }
            Stmt::Yield { value, .. } => self.walk_deterministic_expr(agent, value),
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.walk_deterministic_expr(agent, cond);
                self.walk_deterministic_block(agent, then_block);
                if let Some(eb) = else_block {
                    self.walk_deterministic_block(agent, eb);
                }
            }
            Stmt::For { iter, body, .. } => {
                self.walk_deterministic_expr(agent, iter);
                self.walk_deterministic_block(agent, body);
            }
            Stmt::Expr { expr, .. } => self.walk_deterministic_expr(agent, expr),
            Stmt::Approve { action, span } => {
                // Approve is an LLM-layer concern; a pure function
                // cannot gate on user approval.
                self.errors.push(TypeError::with_guarantee(
                    TypeErrorKind::NonDeterministicCall {
                        agent: agent.to_string(),
                        call: callee_name(action).unwrap_or_else(|| "<approve target>".into()),
                        call_kind: "approve".into(),
                    },
                    *span,
                    "replay.deterministic_pure_path",
                ));
                self.walk_deterministic_expr(agent, action);
            }
        }
    }

    fn walk_deterministic_expr(&mut self, agent: &str, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                self.classify_deterministic_call(agent, callee, *span);
                self.walk_deterministic_expr(agent, callee);
                for arg in args {
                    self.walk_deterministic_expr(agent, arg);
                }
            }
            Expr::FieldAccess { target, .. } | Expr::TryPropagate { inner: target, .. } => {
                self.walk_deterministic_expr(agent, target);
            }
            Expr::Index { target, index, .. } => {
                self.walk_deterministic_expr(agent, target);
                self.walk_deterministic_expr(agent, index);
            }
            Expr::BinOp { left, right, .. } => {
                self.walk_deterministic_expr(agent, left);
                self.walk_deterministic_expr(agent, right);
            }
            Expr::UnOp { operand, .. } => self.walk_deterministic_expr(agent, operand),
            Expr::List { items, .. } => {
                for item in items {
                    self.walk_deterministic_expr(agent, item);
                }
            }
            Expr::TryRetry { body, .. } => self.walk_deterministic_expr(agent, body),
            Expr::Replay {
                trace,
                arms,
                else_body,
                ..
            } => {
                // Walk subexpressions so determinism violations
                // nested inside a replay arm still surface. The
                // `replay` expression itself is treated as pure
                // substrate today Ã¢â‚¬â€ full classification lands with
                // the checker slice (21-inv-E-3).
                self.walk_deterministic_expr(agent, trace);
                for arm in arms {
                    self.walk_deterministic_expr(agent, &arm.body);
                }
                self.walk_deterministic_expr(agent, else_body);
            }
            Expr::Literal { .. } | Expr::Ident { .. } => {}
        }
    }

    /// Classify a call target inside a `@deterministic` body and
    /// emit a `NonDeterministicCall` error if the target fails
    /// the contract. Unresolved or dynamic callees (subscripts,
    /// chained calls) are passed over Ã¢â‚¬â€ they cannot be statically
    /// classified, so the conservative choice is to let the
    /// existing call-check machinery handle them.
    fn classify_deterministic_call(&mut self, agent: &str, callee: &Expr, span: Span) {
        let name = match callee_name(callee) {
            Some(name) => name,
            None => return,
        };

        // Catalog-registered nondeterminism (clocks, PRNGs, etc.)
        // fails `@deterministic` for the same reason it fails
        // `@replayable` Ã¢â‚¬â€ but the error message is stricter.
        if let Some(source) = classify_call_target(&name) {
            self.errors.push(TypeError::with_guarantee(
                TypeErrorKind::NonDeterministicCall {
                    agent: agent.to_string(),
                    call: name.clone(),
                    call_kind: source.label().to_string(),
                },
                span,
                "replay.deterministic_pure_path",
            ));
            return;
        }

        // Resolved decl lookup: if the callee is a bare
        // identifier that binds to a tool / prompt / non-
        // `@deterministic` agent, flag it. Method-call form
        // `x.foo()` is handled by the type checker's method
        // machinery and is deliberately out of scope here for
        // v1 Ã¢â‚¬â€ the catalog + ident-call coverage is enough to
        // enforce the contract on realistic programs; a
        // follow-up slice can extend to method dispatch if
        // users start writing `@deterministic` bodies that
        // route tool calls through receivers.
        let ident_span = match callee {
            Expr::Ident { span, .. } => Some(*span),
            _ => None,
        };
        let binding = ident_span.and_then(|s| self.bindings.get(&s).cloned());
        if let Some(Binding::Decl(def_id)) = binding {
            let entry = self.symbols.get(def_id);
            let call_kind = match entry.kind {
                DeclKind::Tool => Some("tool"),
                DeclKind::Prompt => Some("prompt"),
                DeclKind::Agent => {
                    let callee_agent = self.agents_by_id.get(&def_id).copied();
                    let is_det = callee_agent
                        .map(|a| AgentAttribute::is_deterministic(&a.attributes))
                        .unwrap_or(false);
                    if is_det {
                        None
                    } else {
                        Some("non-`@deterministic` agent")
                    }
                }
                _ => None,
            };
            if let Some(kind) = call_kind {
                self.errors.push(TypeError::with_guarantee(
                    TypeErrorKind::NonDeterministicCall {
                        agent: agent.to_string(),
                        call: name,
                        call_kind: kind.to_string(),
                    },
                    span,
                    "replay.deterministic_pure_path",
                ));
            }
        } else if let Some(Binding::BuiltIn(BuiltIn::Ask | BuiltIn::Choose)) = binding {
            self.errors.push(TypeError::with_guarantee(
                TypeErrorKind::NonDeterministicCall {
                    agent: agent.to_string(),
                    call: name,
                    call_kind: "human".to_string(),
                },
                span,
                "replay.deterministic_pure_path",
            ));
        }
    }
}

// ----------------------------------------------------------------
// Replayability walk helpers (Phase 21 slice 21-inv-A)
// ----------------------------------------------------------------
//
// These walk an agent body and collect `ReplayabilityViolation`
// entries Ã¢â‚¬â€ one per call site that resolves to a nondeterministic
// builtin the trace schema cannot capture. Free functions (not
// methods) so the walk doesn't need `Checker` state; the checker
// pushes the resulting violations into its own error vec.

/// One replayability violation Ã¢â‚¬â€ a call in a `@replayable` body
/// that resolves to a nondeterministic builtin.
struct ReplayabilityViolation {
    call_name: String,
    source: NondeterminismSource,
    span: Span,
}

fn collect_replayability_violations_in_block(block: &Block, out: &mut Vec<ReplayabilityViolation>) {
    for stmt in &block.stmts {
        collect_replayability_violations_in_stmt(stmt, out);
    }
}

fn collect_replayability_violations_in_stmt(stmt: &Stmt, out: &mut Vec<ReplayabilityViolation>) {
    match stmt {
        Stmt::Let { value, .. } => {
            collect_replayability_violations_in_expr(value, out);
        }
        Stmt::Return { value, .. } => {
            if let Some(expr) = value {
                collect_replayability_violations_in_expr(expr, out);
            }
        }
        Stmt::Yield { value, .. } => {
            collect_replayability_violations_in_expr(value, out);
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_replayability_violations_in_expr(cond, out);
            collect_replayability_violations_in_block(then_block, out);
            if let Some(eb) = else_block {
                collect_replayability_violations_in_block(eb, out);
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_replayability_violations_in_expr(iter, out);
            collect_replayability_violations_in_block(body, out);
        }
        Stmt::Expr { expr, .. } => {
            collect_replayability_violations_in_expr(expr, out);
        }
        Stmt::Approve { action, .. } => {
            collect_replayability_violations_in_expr(action, out);
        }
    }
}

fn collect_replayability_violations_in_expr(expr: &Expr, out: &mut Vec<ReplayabilityViolation>) {
    match expr {
        Expr::Call { callee, args, span } => {
            if let Some(name) = callee_name(callee) {
                if let Some(source) = classify_call_target(&name) {
                    out.push(ReplayabilityViolation {
                        call_name: name,
                        source,
                        span: *span,
                    });
                }
            }
            collect_replayability_violations_in_expr(callee, out);
            for arg in args {
                collect_replayability_violations_in_expr(arg, out);
            }
        }
        Expr::FieldAccess { target, .. } | Expr::TryPropagate { inner: target, .. } => {
            collect_replayability_violations_in_expr(target, out);
        }
        Expr::Index { target, index, .. } => {
            collect_replayability_violations_in_expr(target, out);
            collect_replayability_violations_in_expr(index, out);
        }
        Expr::BinOp { left, right, .. } => {
            collect_replayability_violations_in_expr(left, out);
            collect_replayability_violations_in_expr(right, out);
        }
        Expr::UnOp { operand, .. } => {
            collect_replayability_violations_in_expr(operand, out);
        }
        Expr::List { items, .. } => {
            for item in items {
                collect_replayability_violations_in_expr(item, out);
            }
        }
        Expr::TryRetry { body, .. } => {
            collect_replayability_violations_in_expr(body, out);
        }
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            // Walk subexpressions so a replayability violation
            // nested in a replay arm still surfaces. The replay
            // expression itself is replayable-by-construction; its
            // full contract lands with 21-inv-E-3.
            collect_replayability_violations_in_expr(trace, out);
            for arm in arms {
                collect_replayability_violations_in_expr(&arm.body, out);
            }
            collect_replayability_violations_in_expr(else_body, out);
        }
        Expr::Literal { .. } | Expr::Ident { .. } => {}
    }
}

/// Pull a static callee name out of an expression, if the callee
/// is a bare identifier or dotted path. Dynamic callees (subscript,
/// call-returning-call, etc.) return `None`, which the replayability
/// walk treats as "out of catalog scope" Ã¢â‚¬â€ the checker cannot
/// classify them statically, and runtime paths already route
/// through the recorded dispatch layer.
fn callee_name(callee: &Expr) -> Option<String> {
    match callee {
        Expr::Ident { name, .. } => Some(name.name.clone()),
        Expr::FieldAccess { target, field, .. } => {
            let base = callee_name(target)?;
            Some(format!("{base}.{}", field.name))
        }
        _ => None,
    }
}
