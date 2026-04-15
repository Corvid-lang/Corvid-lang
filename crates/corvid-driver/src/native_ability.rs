//! Decides whether an IR file can run through the native AOT tier.
//!
//! Phase 12 ships "native for tool-free programs." Phase 13 adds the
//! async runtime bridge. Phase 14 lifts the `approve` restriction and
//! adds conditional tool-call support: programs with tool calls can
//! compile when the caller supplies a tools staticlib via
//! `--with-tools-lib`; otherwise the scan still reports `ToolCall` so
//! the dispatcher falls back to the interpreter. The scan produces a
//! structured reason so the CLI can tell the user which future slice
//! or phase would lift each remaining restriction.
//!
//! Rationale for a pre-flight IR scan (vs. "try compile, catch
//! NotSupported"): (a) names the native-ability rule explicitly so it's
//! testable and documentable; (b) yields a driver-level error message
//! rather than a codegen-internal one; (c) cheap — O(IR nodes) walk
//! with early exit.

use corvid_ir::{IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrImportSource, IrStmt};

/// Why a program can't run via the native tier. Each variant names the
/// missing feature and the phase that will add it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotNativeReason {
    PythonImport { module: String },
    /// User-declared tool called from compiled code. Phase 14 supports
    /// this via typed-ABI direct calls, but only when the caller
    /// supplies a tools staticlib (`--with-tools-lib`). Without one,
    /// the scan reports this reason and the dispatcher falls back.
    ToolCall { name: String },
    /// Phase 18 language features (Result/Option/`?`/`try-retry`)
    /// parse, typecheck, and interpret fully today. Native codegen
    /// for tagged-union layout and retry desugaring lands in slices
    /// 18d / 18e. Until then the dispatcher routes Phase-18-using
    /// programs to the interpreter tier.
    Phase18Unfinished,
}

impl std::fmt::Display for NotNativeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PythonImport { module } => write!(
                f,
                "program imports Python module `{module}` — native Python FFI lands in Phase 30"
            ),
            Self::ToolCall { name } => write!(
                f,
                "program calls tool `{name}` — pass `--with-tools-lib <path>` pointing at your compiled `#[tool]` staticlib, or let auto-dispatch fall back to the interpreter"
            ),
            Self::Phase18Unfinished => write!(
                f,
                "program uses `Result` / `Option` / `?` / `try ... retry` — native codegen for tagged unions + retry desugaring lands in slices 18d / 18e. Interpreter tier handles it fully today."
            ),
        }
    }
}

/// Walk the IR and return `Ok(())` if every construct is native-able,
/// else the first reason found (early exit — one reason is enough to
/// route the caller to the interpreter tier).
pub fn native_ability(ir: &IrFile) -> Result<(), NotNativeReason> {
    for import in &ir.imports {
        match import.source {
            IrImportSource::Python => {
                return Err(NotNativeReason::PythonImport {
                    module: import.module.clone(),
                });
            }
        }
    }
    for agent in &ir.agents {
        scan_block(&agent.body)?;
    }
    Ok(())
}

fn scan_block(block: &IrBlock) -> Result<(), NotNativeReason> {
    for stmt in &block.stmts {
        scan_stmt(stmt)?;
    }
    Ok(())
}

fn scan_stmt(stmt: &IrStmt) -> Result<(), NotNativeReason> {
    match stmt {
        IrStmt::Let { value, .. } => scan_expr(value),
        IrStmt::Return { value: Some(v), .. } => scan_expr(v),
        IrStmt::Return { value: None, .. } => Ok(()),
        IrStmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            scan_expr(cond)?;
            scan_block(then_block)?;
            if let Some(b) = else_block {
                scan_block(b)?;
            }
            Ok(())
        }
        IrStmt::For { iter, body, .. } => {
            scan_expr(iter)?;
            scan_block(body)
        }
        IrStmt::Approve { args, .. } => {
            // Phase 14: `approve` compiles to a no-op (statically
            // checked by the effect checker; runtime verification
            // lands in Phase 20). Still walk the arg expressions so
            // any tool/prompt call buried in an approve arg is
            // reported.
            for a in args {
                scan_expr(a)?;
            }
            Ok(())
        }
        IrStmt::Expr { expr, .. } => scan_expr(expr),
        IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => Ok(()),
        // Phase 17b: ownership ops contain no user expressions; they
        // don't change whether this agent can run natively.
        IrStmt::Dup { .. } | IrStmt::Drop { .. } => Ok(()),
    }
}

fn scan_expr(expr: &IrExpr) -> Result<(), NotNativeReason> {
    match &expr.kind {
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } => Ok(()),
        IrExprKind::Call {
            kind,
            callee_name,
            args,
        } => {
            match kind {
                IrCallKind::Tool { .. } => {
                    return Err(NotNativeReason::ToolCall {
                        name: callee_name.clone(),
                    })
                }
                IrCallKind::Prompt { .. } => {
                    // Phase 15: prompt calls compile + run natively.
                    // No extra user-provided lib needed (corvid-runtime
                    // ships the LLM adapters built-in). Runtime errors
                    // surface if no provider is configured (no API
                    // key + not Ollama-only).
                }
                IrCallKind::Agent { .. }
                | IrCallKind::StructConstructor { .. }
                | IrCallKind::Unknown => {}
            }
            for a in args {
                scan_expr(a)?;
            }
            Ok(())
        }
        IrExprKind::FieldAccess { target, .. } => scan_expr(target),
        IrExprKind::Index { target, index } => {
            scan_expr(target)?;
            scan_expr(index)
        }
        IrExprKind::BinOp { left, right, .. } => {
            scan_expr(left)?;
            scan_expr(right)
        }
        IrExprKind::UnOp { operand, .. } => scan_expr(operand),
        IrExprKind::List { items } => {
            for it in items {
                scan_expr(it)?;
            }
            Ok(())
        }
        // Phase 18 IR variants — Result/Option/?/try-retry. Until
        // slice 18d lands native codegen for these, the native-
        // ability check routes programs that use them to the
        // interpreter tier. Recurse into sub-expressions so any
        // nested tool/prompt calls inside still get flagged
        // correctly (their non-native reasons take precedence over
        // the Phase-18 gap).
        IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => {
            scan_expr(inner)?;
            Err(NotNativeReason::Phase18Unfinished)
        }
        IrExprKind::OptionNone => Err(NotNativeReason::Phase18Unfinished),
        IrExprKind::TryRetry { body, .. } => {
            scan_expr(body)?;
            Err(NotNativeReason::Phase18Unfinished)
        }
    }
}
