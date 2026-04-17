//! Decides whether an IR file can run through the native AOT tier.
//!
//! The native path currently supports prompt calls and conditionally
//! supports tool calls when the caller supplies a companion tools
//! staticlib via `--with-tools-lib`. The scan produces a structured
//! reason so the CLI can explain why a program falls back to the
//! interpreter.
//!
//! Rationale for a pre-flight IR scan (vs. "try compile, catch
//! NotSupported"): (a) names the native-ability rule explicitly so it's
//! testable and documentable; (b) yields a driver-level error message
//! rather than a codegen-internal one; (c) cheap — O(IR nodes) walk
//! with early exit.

use corvid_ir::{IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrImportSource, IrStmt};
use corvid_types::Type;

/// Why a program can't run via the native tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotNativeReason {
    PythonImport { module: String },
    /// User-declared tool called from compiled code. This is supported
    /// via typed-ABI direct calls, but only when the caller
    /// supplies a tools staticlib (`--with-tools-lib`). Without one,
    /// the scan reports this reason and the dispatcher falls back.
    ToolCall { name: String },
    /// Wider tagged unions and non-Result retry bodies still route to
    /// the interpreter. Nullable-pointer `Option<T>` with a
    /// refcounted payload plus the one-word `Result<T, E>` subset are
    /// the supported native forms today.
    TaggedUnionRetryNotNative,
}

impl std::fmt::Display for NotNativeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PythonImport { module } => write!(
                f,
                "program imports Python module `{module}` — native Python FFI is not implemented yet"
            ),
            Self::ToolCall { name } => write!(
                f,
                "program calls tool `{name}` — pass `--with-tools-lib <path>` pointing at your compiled `#[tool]` staticlib, or let auto-dispatch fall back to the interpreter"
            ),
            Self::TaggedUnionRetryNotNative => write!(
                f,
                "program uses a tagged-union or retry shape outside the current native subset — native AOT supports nullable-pointer `Option<T>`, one-word `Result<T, E>`, postfix `?`, and `try ... retry` over native `Result<T, E>` bodies; wider shapes still run in the interpreter"
            ),
        }
    }
}

fn is_refcounted_type(ty: &Type) -> bool {
    match ty {
        Type::String | Type::Struct(_) | Type::List(_) | Type::Weak(_, _) | Type::Result(_, _) => true,
        Type::Option(inner) => is_refcounted_type(inner),
        _ => false,
    }
}

fn is_native_value_type(ty: &Type) -> bool {
    match ty {
        Type::Int | Type::Bool | Type::Float | Type::String => true,
        Type::Struct(_) | Type::List(_) | Type::Weak(_, _) => true,
        Type::Option(inner) => is_refcounted_type(inner) && is_native_value_type(inner),
        Type::Result(ok, err) => is_native_value_type(ok) && is_native_value_type(err),
        Type::Nothing | Type::Function { .. } | Type::Unknown => false,
    }
}

fn is_native_nullable_option_type(ty: &Type) -> bool {
    matches!(ty, Type::Option(inner) if is_refcounted_type(inner))
}

fn is_native_nullable_option_expr_type(ty: &Type) -> bool {
    matches!(ty, Type::Option(inner) if is_refcounted_type(inner) || matches!(**inner, Type::Unknown))
}

fn is_native_result_type(ty: &Type) -> bool {
    matches!(ty, Type::Result(ok, err) if is_native_value_type(ok) && is_native_value_type(err))
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
        scan_block(&agent.body, &agent.return_ty)?;
    }
    Ok(())
}

fn scan_block(block: &IrBlock, current_return_ty: &Type) -> Result<(), NotNativeReason> {
    for stmt in &block.stmts {
        scan_stmt(stmt, current_return_ty)?;
    }
    Ok(())
}

fn scan_stmt(stmt: &IrStmt, current_return_ty: &Type) -> Result<(), NotNativeReason> {
    match stmt {
        IrStmt::Let { value, .. } => scan_expr(value, current_return_ty),
        IrStmt::Return { value: Some(v), .. } => scan_expr(v, current_return_ty),
        IrStmt::Return { value: None, .. } => Ok(()),
        IrStmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            scan_expr(cond, current_return_ty)?;
            scan_block(then_block, current_return_ty)?;
            if let Some(b) = else_block {
                scan_block(b, current_return_ty)?;
            }
            Ok(())
        }
        IrStmt::For { iter, body, .. } => {
            scan_expr(iter, current_return_ty)?;
            scan_block(body, current_return_ty)
        }
        IrStmt::Approve { args, .. } => {
            // `approve` compiles to a no-op in generated native code.
            // Still walk the arg expressions so any tool/prompt call
            // buried in an approve arg is reported.
            for a in args {
                scan_expr(a, current_return_ty)?;
            }
            Ok(())
        }
        IrStmt::Expr { expr, .. } => scan_expr(expr, current_return_ty),
        IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => Ok(()),
        // Ownership ops contain no user expressions; they don't change
        // whether this agent can run natively.
        IrStmt::Dup { .. } | IrStmt::Drop { .. } => Ok(()),
    }
}

fn scan_expr(expr: &IrExpr, current_return_ty: &Type) -> Result<(), NotNativeReason> {
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
                    // Prompt calls compile and run natively. No extra
                    // user-provided lib is needed because corvid-runtime
                    // ships the LLM adapters built in. Runtime errors
                    // surface if no provider is configured.
                }
                IrCallKind::Agent { .. }
                | IrCallKind::StructConstructor { .. }
                | IrCallKind::Unknown => {}
            }
            for a in args {
                scan_expr(a, current_return_ty)?;
            }
            Ok(())
        }
        IrExprKind::FieldAccess { target, .. } => scan_expr(target, current_return_ty),
        IrExprKind::Index { target, index } => {
            scan_expr(target, current_return_ty)?;
            scan_expr(index, current_return_ty)
        }
        IrExprKind::BinOp { left, right, .. } => {
            scan_expr(left, current_return_ty)?;
            scan_expr(right, current_return_ty)
        }
        IrExprKind::UnOp { operand, .. } => scan_expr(operand, current_return_ty),
        IrExprKind::List { items } => {
            for it in items {
                scan_expr(it, current_return_ty)?;
            }
            Ok(())
        }
        IrExprKind::WeakNew { strong } => scan_expr(strong, current_return_ty),
        IrExprKind::WeakUpgrade { weak } => scan_expr(weak, current_return_ty),
        // Tagged-union/retry nodes are accepted only for the current
        // native subset. Recurse into sub-expressions first so any
        // nested tool/prompt calls still get reported correctly.
        IrExprKind::OptionSome { inner } => {
            scan_expr(inner, current_return_ty)?;
            if is_native_nullable_option_expr_type(&expr.ty) {
                Ok(())
            } else {
                Err(NotNativeReason::TaggedUnionRetryNotNative)
            }
        }
        IrExprKind::ResultOk { inner } | IrExprKind::ResultErr { inner } => {
            scan_expr(inner, current_return_ty)?;
            if is_native_result_type(&expr.ty) {
                Ok(())
            } else {
                Err(NotNativeReason::TaggedUnionRetryNotNative)
            }
        }
        IrExprKind::OptionNone => {
            if is_native_nullable_option_expr_type(&expr.ty) {
                Ok(())
            } else {
                Err(NotNativeReason::TaggedUnionRetryNotNative)
            }
        }
        IrExprKind::TryPropagate { inner } => {
            scan_expr(inner, current_return_ty)?;
            match &inner.ty {
                Type::Option(_) => {
                    if is_native_nullable_option_expr_type(&inner.ty)
                        && is_native_nullable_option_expr_type(current_return_ty)
                    {
                        Ok(())
                    } else {
                        Err(NotNativeReason::TaggedUnionRetryNotNative)
                    }
                }
                Type::Result(_, _) => {
                    if is_native_result_type(&inner.ty) {
                        if let Type::Result(_, outer_err) = current_return_ty {
                            if is_native_result_type(current_return_ty) {
                                if let Type::Result(_, inner_err) = &inner.ty {
                                    if &**outer_err == &**inner_err {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                    Err(NotNativeReason::TaggedUnionRetryNotNative)
                }
                _ => Err(NotNativeReason::TaggedUnionRetryNotNative),
            }
        }
        IrExprKind::TryRetry { body, .. } => {
            scan_expr(body, current_return_ty)?;
            if &body.ty == &expr.ty && is_native_result_type(&body.ty) {
                Ok(())
            } else {
                Err(NotNativeReason::TaggedUnionRetryNotNative)
            }
        }
    }
}
