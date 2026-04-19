//! Agent + eval declaration checking.
//!
//! `check_agent` validates an `agent name(params) -> T: body` —
//! parameter binding, return-type matching, yield/stream legality.
//! `check_eval` validates an `eval name: body` — including
//! trace-assert (`assert called X before Y`) and statistical
//! confidence modifiers.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::Checker;
use crate::determinism::{classify_call_target, NondeterminismSource};
use crate::errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
use crate::types::Type;
use corvid_ast::{AgentAttribute, AgentDecl, Block, EvalAssert, EvalDecl, Expr, Ident, Span, Stmt};
use corvid_resolve::{Binding, DeclKind};

impl<'a> Checker<'a> {
    pub(super) fn check_agent(&mut self, a: &AgentDecl) {
        // Bind parameter types.
        self.bind_params(&a.params);

        let declared_ret = self.type_ref_to_type(&a.return_ty);
        let prev_ret = std::mem::replace(&mut self.current_return, Some(declared_ret.clone()));
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, true);
        let prev_saw_yield = std::mem::replace(&mut self.saw_yield, false);

        self.check_block(&a.body);

        if matches!(declared_ret, Type::Stream(_)) && !self.saw_yield {
            self.warnings.push(TypeWarning::new(
                TypeWarningKind::StreamReturnWithoutYield {
                    agent: a.name.name.clone(),
                },
                a.span,
            ));
        }

        // Phase 21 slice 21-inv-A: enforce `@replayable`. An agent
        // carrying the attribute must call only functions whose
        // outputs the trace schema can capture; anything in the
        // determinism catalog (clocks, PRNGs, environment reads,
        // etc.) is off-limits.
        //
        // The catalog is empty as of Phase 21 v1 because Corvid
        // source does not yet expose any nondeterministic builtins.
        // The walk runs anyway so the enforcement path is live and
        // ready to fire the moment an entry lands.
        if a.attributes
            .iter()
            .any(|attr| matches!(attr, AgentAttribute::Replayable { .. }))
        {
            self.check_replayable_body(&a.name.name, &a.body);
        }

        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.saw_yield = prev_saw_yield;
        // (Locals leak between agents in our single-scope model; harmless
        //  since each agent binds its params fresh at the start.)
    }

    /// Walk `body` looking for calls to functions the determinism
    /// catalog flags as nondeterministic. Emits one
    /// `NonReplayableCall` error per offending call site. Safe to
    /// call on any body — a catalog-empty walk is a no-op.
    fn check_replayable_body(&mut self, agent_name: &str, body: &Block) {
        let mut violations = Vec::new();
        collect_replayability_violations_in_block(body, &mut violations);
        for violation in violations {
            self.errors.push(TypeError::new(
                TypeErrorKind::NonReplayableCall {
                    agent: agent_name.to_string(),
                    call: violation.call_name,
                    source_label: violation.source.label().to_string(),
                },
                violation.span,
            ));
        }
    }


    pub(super) fn check_eval(&mut self, e: &EvalDecl) {
        let prev_ret = self.current_return.take();
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, false);
        let prev_saw_yield = self.saw_yield;
        self.check_block(&e.body);
        for assertion in &e.assertions {
            self.check_eval_assert(assertion);
        }
        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.saw_yield = prev_saw_yield;
    }

    fn check_eval_assert(&mut self, assertion: &EvalAssert) {
        match assertion {
            EvalAssert::Value {
                expr,
                confidence,
                runs,
                span,
            } => {
                let ty = self.check_expr(expr);
                if !matches!(ty, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AssertNotBool {
                            got: ty.display_name(),
                        },
                        *span,
                    ));
                }
                if let Some(value) = confidence {
                    if !(0.0..=1.0).contains(value) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::InvalidConfidence { value: *value },
                            *span,
                        ));
                    }
                }
                if matches!(runs, Some(0)) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidConfidence { value: 0.0 },
                        *span,
                    ));
                }
            }
            EvalAssert::Called { tool, span } => {
                self.check_eval_callable(tool, *span);
            }
            EvalAssert::Approved { label, span } => {
                if !self.has_known_approval_label(&label.name) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::EvalUnknownApproval {
                            label: label.name.clone(),
                        },
                        *span,
                    ));
                }
            }
            EvalAssert::Cost { .. } => {}
            EvalAssert::Ordering {
                before,
                after,
                span,
            } => {
                self.check_eval_callable(before, *span);
                self.check_eval_callable(after, *span);
            }
        }
    }

    fn check_eval_callable(&mut self, ident: &Ident, span: Span) {
        match self.bindings.get(&ident.span) {
            Some(Binding::Decl(def_id)) => match self.symbols.get(*def_id).kind {
                DeclKind::Tool | DeclKind::Prompt | DeclKind::Agent => {}
                _ => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::EvalUnknownTool {
                            name: ident.name.clone(),
                        },
                        span,
                    ));
                }
            },
            _ => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::EvalUnknownTool {
                        name: ident.name.clone(),
                    },
                    span,
                ));
            }
        }
    }
}

// ----------------------------------------------------------------
// Replayability walk helpers (Phase 21 slice 21-inv-A)
// ----------------------------------------------------------------
//
// These walk an agent body and collect `ReplayabilityViolation`
// entries — one per call site that resolves to a nondeterministic
// builtin the trace schema cannot capture. Free functions (not
// methods) so the walk doesn't need `Checker` state; the checker
// pushes the resulting violations into its own error vec.

/// One replayability violation — a call in a `@replayable` body
/// that resolves to a nondeterministic builtin.
struct ReplayabilityViolation {
    call_name: String,
    source: NondeterminismSource,
    span: Span,
}

fn collect_replayability_violations_in_block(
    block: &Block,
    out: &mut Vec<ReplayabilityViolation>,
) {
    for stmt in &block.stmts {
        collect_replayability_violations_in_stmt(stmt, out);
    }
}

fn collect_replayability_violations_in_stmt(
    stmt: &Stmt,
    out: &mut Vec<ReplayabilityViolation>,
) {
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

fn collect_replayability_violations_in_expr(
    expr: &Expr,
    out: &mut Vec<ReplayabilityViolation>,
) {
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
        Expr::Literal { .. } | Expr::Ident { .. } => {}
    }
}

/// Pull a static callee name out of an expression, if the callee
/// is a bare identifier or dotted path. Dynamic callees (subscript,
/// call-returning-call, etc.) return `None`, which the replayability
/// walk treats as "out of catalog scope" — the checker cannot
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
