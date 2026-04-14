//! Decides whether an IR file can run through the native AOT tier.
//!
//! Phase 12 ships "native for tool-free programs." Anything that needs
//! the async interpreter or runtime (tool calls, prompt calls, `approve`,
//! Python imports) falls back to the interpreter. The scan produces a
//! structured reason so the CLI can tell the user which future slice or
//! phase would lift each restriction.
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
    ToolCall { name: String },
    PromptCall { name: String },
    Approve { label: String },
}

impl std::fmt::Display for NotNativeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PythonImport { module } => write!(
                f,
                "program imports Python module `{module}` — native Python FFI lands in Phase 16"
            ),
            Self::ToolCall { name } => write!(
                f,
                "program calls tool `{name}` — native tool dispatch lands in Phase 14"
            ),
            Self::PromptCall { name } => write!(
                f,
                "program calls prompt `{name}` — native prompt dispatch lands in Phase 14"
            ),
            Self::Approve { label } => write!(
                f,
                "program uses `approve {label}` — native approve handling lands in Phase 14"
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
        IrStmt::Approve { label, args, .. } => {
            for a in args {
                scan_expr(a)?;
            }
            Err(NotNativeReason::Approve {
                label: label.clone(),
            })
        }
        IrStmt::Expr { expr, .. } => scan_expr(expr),
        IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => Ok(()),
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
                    return Err(NotNativeReason::PromptCall {
                        name: callee_name.clone(),
                    })
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
    }
}
