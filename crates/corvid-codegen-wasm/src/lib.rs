//! WebAssembly code generator.
//!
//! Phase 23 starts with a deliberately honest deployment surface:
//! scalar, runtime-free agents compile to a standalone `.wasm` module
//! plus JS and TypeScript companions. AI-native host imports for LLMs,
//! tools, approvals, replay recording, and provenance are follow-up
//! slices because they need a real browser/edge host-capability ABI.

use corvid_ast::{BinaryOp, UnaryOp};
use corvid_ir::{IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrLiteral, IrStmt};
use corvid_resolve::{DefId, LocalId};
use corvid_types::Type;
use std::collections::HashMap;
use wasm_encoder::{
    BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
    Module, TypeSection, ValType,
};

mod companions;
mod error;

pub use companions::WasmArtifacts;
pub use error::WasmCodegenError;

pub fn emit_wasm_artifacts(ir: &IrFile, module_name: &str) -> Result<WasmArtifacts, WasmCodegenError> {
    let scalar_agents = ir
        .agents
        .iter()
        .map(validate_agent)
        .collect::<Result<Vec<_>, _>>()?;

    let mut agent_indices = HashMap::new();
    for (idx, agent) in scalar_agents.iter().enumerate() {
        agent_indices.insert(agent.id, idx as u32);
    }

    let mut types = TypeSection::new();
    let mut funcs = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();

    for agent in &scalar_agents {
        let params = agent
            .params
            .iter()
            .map(|param| wasm_val_type(&param.ty))
            .collect::<Result<Vec<_>, _>>()?;
        let results = wasm_result_types(&agent.return_ty)?;
        let type_index = types.len();
        types.ty().function(params, results);
        funcs.function(type_index);
    }

    for (idx, agent) in scalar_agents.iter().enumerate() {
        exports.export(&agent.name, ExportKind::Func, idx as u32);
        let function = compile_agent(agent, &agent_indices)?;
        code.function(&function);
    }

    let mut module = Module::new();
    module.section(&types);
    module.section(&funcs);
    module.section(&exports);
    module.section(&code);

    companions::build_artifacts(module_name, &scalar_agents, module.finish())
}

fn validate_agent(agent: &IrAgent) -> Result<&IrAgent, WasmCodegenError> {
    if agent.extern_abi.is_some() {
        return Err(WasmCodegenError::unsupported(format!(
            "wasm target does not lower `pub extern \"c\"` agent `{}`; export a normal agent for browser/edge use",
            agent.name
        )));
    }
    for param in &agent.params {
        wasm_val_type(&param.ty).map_err(|_| {
            WasmCodegenError::unsupported(format!(
                "wasm target currently supports only Int, Float, Bool, and Nothing scalar parameters; agent `{}` parameter `{}` has `{}`",
                agent.name,
                param.name,
                param.ty.display_name()
            ))
        })?;
    }
    wasm_result_types(&agent.return_ty).map_err(|_| {
        WasmCodegenError::unsupported(format!(
            "wasm target currently supports only Int, Float, Bool, and Nothing scalar returns; agent `{}` returns `{}`",
            agent.name,
            agent.return_ty.display_name()
        ))
    })?;
    reject_runtime_calls(&agent.body, &agent.name)?;
    Ok(agent)
}

fn reject_runtime_calls(block: &IrBlock, agent_name: &str) -> Result<(), WasmCodegenError> {
    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let { value, .. } | IrStmt::Expr { expr: value, .. } => {
                reject_runtime_expr(value, agent_name)?;
            }
            IrStmt::Return { value, .. } => {
                if let Some(value) = value {
                    reject_runtime_expr(value, agent_name)?;
                }
            }
            IrStmt::Yield { value, .. } => {
                reject_runtime_expr(value, agent_name)?;
            }
            IrStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                reject_runtime_expr(cond, agent_name)?;
                reject_runtime_calls(then_block, agent_name)?;
                if let Some(else_block) = else_block {
                    reject_runtime_calls(else_block, agent_name)?;
                }
            }
            IrStmt::For { .. } => {
                return Err(WasmCodegenError::unsupported(format!(
                    "wasm target does not yet lower loops in agent `{agent_name}`"
                )));
            }
            IrStmt::Approve { .. } => {
                return Err(WasmCodegenError::unsupported(format!(
                    "wasm target needs the Phase 23 host approval ABI before lowering approvals in agent `{agent_name}`"
                )));
            }
            IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => {}
            IrStmt::Dup { .. } | IrStmt::Drop { .. } => {}
        }
    }
    Ok(())
}

fn reject_runtime_expr(expr: &IrExpr, agent_name: &str) -> Result<(), WasmCodegenError> {
    match &expr.kind {
        IrExprKind::Call { kind, args, callee_name } => {
            for arg in args {
                reject_runtime_expr(arg, agent_name)?;
            }
            match kind {
                IrCallKind::Agent { .. } => Ok(()),
                IrCallKind::Tool { .. } => Err(WasmCodegenError::unsupported(format!(
                    "wasm target needs the Phase 23 host tool ABI before lowering tool call `{callee_name}` in agent `{agent_name}`"
                ))),
                IrCallKind::Prompt { .. } => Err(WasmCodegenError::unsupported(format!(
                    "wasm target needs the Phase 23 host LLM ABI before lowering prompt call `{callee_name}` in agent `{agent_name}`"
                ))),
                IrCallKind::StructConstructor { .. } | IrCallKind::Unknown => Err(
                    WasmCodegenError::unsupported(format!(
                        "wasm target currently supports scalar runtime-free agents; call `{callee_name}` in agent `{agent_name}` is not scalar"
                    )),
                ),
            }
        }
        IrExprKind::BinOp { left, right, .. } | IrExprKind::WrappingBinOp { left, right, .. } => {
            reject_runtime_expr(left, agent_name)?;
            reject_runtime_expr(right, agent_name)
        }
        IrExprKind::UnOp { operand, .. } | IrExprKind::WrappingUnOp { operand, .. } => {
            reject_runtime_expr(operand, agent_name)
        }
        IrExprKind::FieldAccess { target, .. }
        | IrExprKind::Index { target, .. }
        | IrExprKind::UnwrapGrounded { value: target }
        | IrExprKind::WeakNew { strong: target }
        | IrExprKind::WeakUpgrade { weak: target }
        | IrExprKind::StreamSplitBy { stream: target, .. }
        | IrExprKind::StreamMerge { groups: target, .. }
        | IrExprKind::StreamOrderedBy { stream: target, .. }
        | IrExprKind::StreamResumeToken { stream: target }
        | IrExprKind::ResumeStream { token: target, .. }
        | IrExprKind::ResultOk { inner: target }
        | IrExprKind::ResultErr { inner: target }
        | IrExprKind::OptionSome { inner: target }
        | IrExprKind::TryPropagate { inner: target } => reject_runtime_expr(target, agent_name),
        IrExprKind::TryRetry { body, .. } => reject_runtime_expr(body, agent_name),
        IrExprKind::List { items } => {
            for item in items {
                reject_runtime_expr(item, agent_name)?;
            }
            Ok(())
        }
        IrExprKind::Replay {
            trace,
            arms,
            else_body,
        } => {
            reject_runtime_expr(trace, agent_name)?;
            for arm in arms {
                reject_runtime_expr(&arm.body, agent_name)?;
            }
            reject_runtime_expr(else_body, agent_name)
        }
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } | IrExprKind::OptionNone => Ok(()),
    }
}

fn compile_agent(
    agent: &IrAgent,
    agent_indices: &HashMap<DefId, u32>,
) -> Result<Function, WasmCodegenError> {
    let mut locals = LocalLayout::from_agent(agent)?;
    collect_block_locals(&agent.body, &mut locals)?;
    let local_groups = locals.local_groups();
    let mut function = Function::new(local_groups);

    emit_block(&agent.body, &mut function, &locals, agent_indices)?;
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    Ok(function)
}

fn collect_block_locals(block: &IrBlock, locals: &mut LocalLayout) -> Result<(), WasmCodegenError> {
    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let {
                local_id, ty, name, ..
            } => locals.add_local(*local_id, name, ty)?,
            IrStmt::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_locals(then_block, locals)?;
                if let Some(else_block) = else_block {
                    collect_block_locals(else_block, locals)?;
                }
            }
            IrStmt::For { .. } => {
                return Err(WasmCodegenError::unsupported(
                    "wasm target does not yet lower loop locals",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

struct LocalLayout {
    map: HashMap<LocalId, u32>,
    locals: Vec<(String, ValType)>,
}

impl LocalLayout {
    fn from_agent(agent: &IrAgent) -> Result<Self, WasmCodegenError> {
        let mut layout = Self {
            map: HashMap::new(),
            locals: Vec::new(),
        };
        for (idx, param) in agent.params.iter().enumerate() {
            layout.map.insert(param.local_id, idx as u32);
        }
        Ok(layout)
    }

    fn add_local(
        &mut self,
        local_id: LocalId,
        name: &str,
        ty: &Type,
    ) -> Result<(), WasmCodegenError> {
        if self.map.contains_key(&local_id) {
            return Ok(());
        }
        let wasm_ty = wasm_val_type(ty).map_err(|_| {
            WasmCodegenError::unsupported(format!(
                "wasm target local `{name}` has unsupported type `{}`",
                ty.display_name()
            ))
        })?;
        let index = self.map.len() as u32;
        self.map.insert(local_id, index);
        self.locals.push((name.to_string(), wasm_ty));
        Ok(())
    }

    fn local_groups(&self) -> Vec<(u32, ValType)> {
        self.locals.iter().map(|(_, ty)| (1, *ty)).collect()
    }

    fn index(&self, local_id: LocalId, name: &str) -> Result<u32, WasmCodegenError> {
        self.map.get(&local_id).copied().ok_or_else(|| {
            WasmCodegenError::unsupported(format!(
                "wasm target could not resolve local `{name}`"
            ))
        })
    }
}

fn emit_block(
    block: &IrBlock,
    function: &mut Function,
    locals: &LocalLayout,
    agent_indices: &HashMap<DefId, u32>,
) -> Result<(), WasmCodegenError> {
    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let {
                local_id,
                name,
                value,
                ..
            } => {
                emit_expr(value, function, locals, agent_indices)?;
                function.instruction(&Instruction::LocalSet(locals.index(*local_id, name)?));
            }
            IrStmt::Return { value, .. } => {
                if let Some(value) = value {
                    emit_expr(value, function, locals, agent_indices)?;
                }
                function.instruction(&Instruction::Return);
            }
            IrStmt::Expr { expr, .. } => {
                emit_expr(expr, function, locals, agent_indices)?;
                if !matches!(expr.ty, Type::Nothing) {
                    function.instruction(&Instruction::Drop);
                }
            }
            IrStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                emit_expr(cond, function, locals, agent_indices)?;
                function.instruction(&Instruction::If(BlockType::Empty));
                emit_block(then_block, function, locals, agent_indices)?;
                if let Some(else_block) = else_block {
                    function.instruction(&Instruction::Else);
                    emit_block(else_block, function, locals, agent_indices)?;
                }
                function.instruction(&Instruction::End);
            }
            IrStmt::Pass { .. } | IrStmt::Dup { .. } | IrStmt::Drop { .. } => {}
            IrStmt::Yield { .. }
            | IrStmt::For { .. }
            | IrStmt::Approve { .. }
            | IrStmt::Break { .. }
            | IrStmt::Continue { .. } => {
                return Err(WasmCodegenError::unsupported(format!(
                    "wasm target cannot lower statement `{stmt:?}` yet"
                )));
            }
        }
    }
    Ok(())
}

fn emit_expr(
    expr: &IrExpr,
    function: &mut Function,
    locals: &LocalLayout,
    agent_indices: &HashMap<DefId, u32>,
) -> Result<(), WasmCodegenError> {
    match &expr.kind {
        IrExprKind::Literal(IrLiteral::Int(value)) => {
            function.instruction(&Instruction::I64Const(*value));
        }
        IrExprKind::Literal(IrLiteral::Float(value)) => {
            function.instruction(&Instruction::F64Const((*value).into()));
        }
        IrExprKind::Literal(IrLiteral::Bool(value)) => {
            function.instruction(&Instruction::I32Const(i32::from(*value)));
        }
        IrExprKind::Literal(IrLiteral::String(_)) => {
            return Err(WasmCodegenError::unsupported(
                "wasm target needs the Phase 23 string ABI before lowering String literals",
            ));
        }
        IrExprKind::Literal(IrLiteral::Nothing) => {}
        IrExprKind::Local { local_id, name } => {
            function.instruction(&Instruction::LocalGet(locals.index(*local_id, name)?));
        }
        IrExprKind::Call {
            kind,
            args,
            callee_name,
        } => match kind {
            IrCallKind::Agent { def_id } => {
                for arg in args {
                    emit_expr(arg, function, locals, agent_indices)?;
                }
                let index = agent_indices.get(def_id).copied().ok_or_else(|| {
                    WasmCodegenError::unsupported(format!(
                        "wasm target could not resolve agent call `{callee_name}`"
                    ))
                })?;
                function.instruction(&Instruction::Call(index));
            }
            _ => {
                return Err(WasmCodegenError::unsupported(format!(
                    "wasm target cannot lower runtime call `{callee_name}` without host-capability ABI"
                )));
            }
        },
        IrExprKind::BinOp { op, left, right } | IrExprKind::WrappingBinOp { op, left, right } => {
            emit_expr(left, function, locals, agent_indices)?;
            emit_expr(right, function, locals, agent_indices)?;
            emit_binary(*op, &left.ty, function)?;
        }
        IrExprKind::UnOp { op, operand } | IrExprKind::WrappingUnOp { op, operand } => {
            emit_unary(*op, operand, function, locals, agent_indices)?;
        }
        IrExprKind::Decl { .. }
        | IrExprKind::FieldAccess { .. }
        | IrExprKind::Index { .. }
        | IrExprKind::List { .. }
        | IrExprKind::UnwrapGrounded { .. }
        | IrExprKind::WeakNew { .. }
        | IrExprKind::WeakUpgrade { .. }
        | IrExprKind::StreamSplitBy { .. }
        | IrExprKind::StreamMerge { .. }
        | IrExprKind::StreamOrderedBy { .. }
        | IrExprKind::StreamResumeToken { .. }
        | IrExprKind::ResumeStream { .. }
        | IrExprKind::ResultOk { .. }
        | IrExprKind::ResultErr { .. }
        | IrExprKind::OptionSome { .. }
        | IrExprKind::OptionNone
        | IrExprKind::TryPropagate { .. }
        | IrExprKind::TryRetry { .. }
        | IrExprKind::Replay { .. } => {
            return Err(WasmCodegenError::unsupported(format!(
                "wasm target cannot lower expression `{expr:?}` yet"
            )));
        }
    }
    Ok(())
}

fn emit_binary(
    op: BinaryOp,
    operand_ty: &Type,
    function: &mut Function,
) -> Result<(), WasmCodegenError> {
    let instruction = match (op, operand_ty) {
        (BinaryOp::Add, Type::Int) => Instruction::I64Add,
        (BinaryOp::Sub, Type::Int) => Instruction::I64Sub,
        (BinaryOp::Mul, Type::Int) => Instruction::I64Mul,
        (BinaryOp::Div, Type::Int) => Instruction::I64DivS,
        (BinaryOp::Mod, Type::Int) => Instruction::I64RemS,
        (BinaryOp::Add, Type::Float) => Instruction::F64Add,
        (BinaryOp::Sub, Type::Float) => Instruction::F64Sub,
        (BinaryOp::Mul, Type::Float) => Instruction::F64Mul,
        (BinaryOp::Div, Type::Float) => Instruction::F64Div,
        (BinaryOp::Eq, Type::Int) => Instruction::I64Eq,
        (BinaryOp::NotEq, Type::Int) => Instruction::I64Ne,
        (BinaryOp::Lt, Type::Int) => Instruction::I64LtS,
        (BinaryOp::LtEq, Type::Int) => Instruction::I64LeS,
        (BinaryOp::Gt, Type::Int) => Instruction::I64GtS,
        (BinaryOp::GtEq, Type::Int) => Instruction::I64GeS,
        (BinaryOp::Eq, Type::Float) => Instruction::F64Eq,
        (BinaryOp::NotEq, Type::Float) => Instruction::F64Ne,
        (BinaryOp::Lt, Type::Float) => Instruction::F64Lt,
        (BinaryOp::LtEq, Type::Float) => Instruction::F64Le,
        (BinaryOp::Gt, Type::Float) => Instruction::F64Gt,
        (BinaryOp::GtEq, Type::Float) => Instruction::F64Ge,
        (BinaryOp::Eq, Type::Bool) => Instruction::I32Eq,
        (BinaryOp::NotEq, Type::Bool) => Instruction::I32Ne,
        (BinaryOp::And, Type::Bool) => Instruction::I32And,
        (BinaryOp::Or, Type::Bool) => Instruction::I32Or,
        _ => {
            return Err(WasmCodegenError::unsupported(format!(
                "wasm target cannot lower binary op `{op:?}` for `{}`",
                operand_ty.display_name()
            )));
        }
    };
    function.instruction(&instruction);
    Ok(())
}

fn emit_unary(
    op: UnaryOp,
    operand: &IrExpr,
    function: &mut Function,
    locals: &LocalLayout,
    agent_indices: &HashMap<DefId, u32>,
) -> Result<(), WasmCodegenError> {
    match (op, &operand.ty) {
        (UnaryOp::Neg, Type::Int) => {
            function.instruction(&Instruction::I64Const(0));
            emit_expr(operand, function, locals, agent_indices)?;
            function.instruction(&Instruction::I64Sub);
        }
        (UnaryOp::Neg, Type::Float) => {
            emit_expr(operand, function, locals, agent_indices)?;
            function.instruction(&Instruction::F64Neg);
        }
        (UnaryOp::Not, Type::Bool) => {
            emit_expr(operand, function, locals, agent_indices)?;
            function.instruction(&Instruction::I32Eqz);
        }
        _ => {
            return Err(WasmCodegenError::unsupported(format!(
                "wasm target cannot lower unary op `{op:?}` for `{}`",
                operand.ty.display_name()
            )));
        }
    }
    Ok(())
}

fn wasm_val_type(ty: &Type) -> Result<ValType, WasmCodegenError> {
    match ty {
        Type::Int => Ok(ValType::I64),
        Type::Float => Ok(ValType::F64),
        Type::Bool => Ok(ValType::I32),
        _ => Err(WasmCodegenError::unsupported(format!(
            "unsupported wasm scalar type `{}`",
            ty.display_name()
        ))),
    }
}

fn wasm_result_types(ty: &Type) -> Result<Vec<ValType>, WasmCodegenError> {
    if matches!(ty, Type::Nothing) {
        Ok(Vec::new())
    } else {
        Ok(vec![wasm_val_type(ty)?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ir::lower;
    use corvid_resolve::resolve;
    use corvid_syntax::{lex, parse_file};
    use corvid_types::typecheck;

    fn lower_src(src: &str) -> IrFile {
        let tokens = lex(src).expect("lex");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse: {perr:?}");
        let resolved = resolve(&file);
        assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
        let checked = typecheck(&file, &resolved);
        assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
        lower(&file, &resolved, &checked)
    }

    #[test]
    fn emits_valid_wasm_for_scalar_agent() {
        let ir = lower_src(
            r#"
agent add_one(x: Int) -> Int:
    y = x + 1
    return y
"#,
        );
        let artifacts = emit_wasm_artifacts(&ir, "math").expect("wasm artifacts");
        wasmparser::Validator::new()
            .validate_all(&artifacts.wasm)
            .expect("valid wasm");
        assert!(artifacts.js_loader.contains("add_one(x)"));
        assert!(artifacts.ts_types.contains("add_one(x: bigint): bigint"));
        assert!(artifacts.manifest_json.contains("\"module_name\": \"math\""));
    }

    #[test]
    fn rejects_prompt_until_host_capability_abi_exists() {
        let ir = lower_src(
            r#"
prompt answer() -> Int:
    """Return 42."""

agent main() -> Int:
    return answer()
"#,
        );
        let err = emit_wasm_artifacts(&ir, "prompted").expect_err("prompt unsupported");
        assert!(err.message.contains("host LLM ABI"));
    }
}
