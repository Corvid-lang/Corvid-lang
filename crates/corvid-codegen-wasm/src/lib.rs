//! WebAssembly code generator.
//!
//! Phase 23 starts with a deliberately honest deployment surface:
//! scalar, runtime-free agents compile to a standalone `.wasm` module
//! plus JS and TypeScript companions. AI-native host imports for LLMs,
//! tools, approvals, replay recording, and provenance are follow-up
//! slices because they need a real browser/edge host-capability ABI.

use corvid_ast::{BinaryOp, UnaryOp};
use corvid_ir::{
    IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrLiteral, IrPrompt, IrStmt, IrTool,
};
use corvid_resolve::{DefId, LocalId};
use corvid_types::Type;
use std::collections::HashMap;
use wasm_encoder::{
    BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    Instruction, Module, TypeSection, ValType,
};

mod companions;
mod error;

pub use companions::WasmArtifacts;
pub use error::WasmCodegenError;

const HOST_MODULE: &str = "corvid:host";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostImportKind {
    Prompt,
    Tool,
    Approval,
}

impl HostImportKind {
    pub(crate) fn namespace(self) -> &'static str {
        match self {
            HostImportKind::Prompt => "prompts",
            HostImportKind::Tool => "tools",
            HostImportKind::Approval => "approvals",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            HostImportKind::Prompt => "prompt",
            HostImportKind::Tool => "tool",
            HostImportKind::Approval => "approval",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WasmHostImport {
    pub kind: HostImportKind,
    pub source_name: String,
    pub import_name: String,
    pub params: Vec<(String, Type)>,
    pub return_ty: Type,
}

struct HostImportPlan {
    imports: Vec<WasmHostImport>,
    tool_indices: HashMap<DefId, u32>,
    prompt_indices: HashMap<DefId, u32>,
    approval_indices: HashMap<String, u32>,
}

pub fn emit_wasm_artifacts(ir: &IrFile, module_name: &str) -> Result<WasmArtifacts, WasmCodegenError> {
    let scalar_agents = ir
        .agents
        .iter()
        .map(validate_agent)
        .collect::<Result<Vec<_>, _>>()?;
    let host_plan = collect_host_imports(ir, &scalar_agents)?;

    let mut agent_indices = HashMap::new();
    for (idx, agent) in scalar_agents.iter().enumerate() {
        agent_indices.insert(agent.id, host_plan.imports.len() as u32 + idx as u32);
    }

    let mut types = TypeSection::new();
    let mut imports = ImportSection::new();
    let mut funcs = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();

    for host_import in &host_plan.imports {
        let params = host_import
            .params
            .iter()
            .map(|(_, ty)| wasm_val_type(ty))
            .collect::<Result<Vec<_>, _>>()?;
        let results = wasm_result_types(&host_import.return_ty)?;
        let type_index = types.len();
        types.ty().function(params, results);
        imports.import(
            HOST_MODULE,
            &host_import.import_name,
            wasm_encoder::EntityType::Function(type_index),
        );
    }

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
        exports.export(
            &agent.name,
            ExportKind::Func,
            host_plan.imports.len() as u32 + idx as u32,
        );
        let function = compile_agent(agent, &agent_indices, &host_plan)?;
        code.function(&function);
    }

    let mut module = Module::new();
    module.section(&types);
    if !host_plan.imports.is_empty() {
        module.section(&imports);
    }
    module.section(&funcs);
    module.section(&exports);
    module.section(&code);

    companions::build_artifacts(module_name, &scalar_agents, &host_plan.imports, module.finish())
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
    Ok(agent)
}

fn collect_host_imports(
    ir: &IrFile,
    agents: &[&IrAgent],
) -> Result<HostImportPlan, WasmCodegenError> {
    let tools = ir
        .tools
        .iter()
        .map(|tool| (tool.id, tool))
        .collect::<HashMap<_, _>>();
    let prompts = ir
        .prompts
        .iter()
        .map(|prompt| (prompt.id, prompt))
        .collect::<HashMap<_, _>>();
    let mut plan = HostImportPlan {
        imports: Vec::new(),
        tool_indices: HashMap::new(),
        prompt_indices: HashMap::new(),
        approval_indices: HashMap::new(),
    };
    for agent in agents {
        collect_block_imports(&agent.body, &tools, &prompts, &mut plan, &agent.name)?;
    }
    Ok(plan)
}

fn collect_block_imports(
    block: &IrBlock,
    tools: &HashMap<DefId, &IrTool>,
    prompts: &HashMap<DefId, &IrPrompt>,
    plan: &mut HostImportPlan,
    agent_name: &str,
) -> Result<(), WasmCodegenError> {
    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let { value, .. } | IrStmt::Expr { expr: value, .. } => {
                collect_expr_imports(value, tools, prompts, plan, agent_name)?;
            }
            IrStmt::Return { value, .. } => {
                if let Some(value) = value {
                    collect_expr_imports(value, tools, prompts, plan, agent_name)?;
                }
            }
            IrStmt::Yield { value, .. } => {
                collect_expr_imports(value, tools, prompts, plan, agent_name)?;
            }
            IrStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                collect_expr_imports(cond, tools, prompts, plan, agent_name)?;
                collect_block_imports(then_block, tools, prompts, plan, agent_name)?;
                if let Some(else_block) = else_block {
                    collect_block_imports(else_block, tools, prompts, plan, agent_name)?;
                }
            }
            IrStmt::For { .. } => {
                return Err(WasmCodegenError::unsupported(format!(
                    "wasm target does not yet lower loops in agent `{agent_name}`"
                )));
            }
            IrStmt::Approve { label, args, .. } => {
                for arg in args {
                    collect_expr_imports(arg, tools, prompts, plan, agent_name)?;
                    wasm_val_type(&arg.ty).map_err(|_| {
                        WasmCodegenError::unsupported(format!(
                            "wasm approval `{label}` in agent `{agent_name}` has unsupported argument type `{}`",
                            arg.ty.display_name()
                        ))
                    })?;
                }
                add_approval_import(plan, label, args)?;
            }
            IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => {}
            IrStmt::Dup { .. } | IrStmt::Drop { .. } => {}
        }
    }
    Ok(())
}

fn collect_expr_imports(
    expr: &IrExpr,
    tools: &HashMap<DefId, &IrTool>,
    prompts: &HashMap<DefId, &IrPrompt>,
    plan: &mut HostImportPlan,
    agent_name: &str,
) -> Result<(), WasmCodegenError> {
    match &expr.kind {
        IrExprKind::Call { kind, args, callee_name } => {
            for arg in args {
                collect_expr_imports(arg, tools, prompts, plan, agent_name)?;
            }
            match kind {
                IrCallKind::Agent { .. } => Ok(()),
                IrCallKind::Tool { def_id, .. } => {
                    let tool = tools.get(def_id).ok_or_else(|| {
                        WasmCodegenError::unsupported(format!(
                            "wasm target could not resolve tool import `{callee_name}`"
                        ))
                    })?;
                    add_tool_import(plan, tool)
                }
                IrCallKind::Prompt { def_id } => {
                    let prompt = prompts.get(def_id).ok_or_else(|| {
                        WasmCodegenError::unsupported(format!(
                            "wasm target could not resolve prompt import `{callee_name}`"
                        ))
                    })?;
                    add_prompt_import(plan, prompt)
                }
                IrCallKind::StructConstructor { .. } | IrCallKind::Unknown => Err(
                    WasmCodegenError::unsupported(format!(
                        "wasm target currently supports scalar runtime-free agents; call `{callee_name}` in agent `{agent_name}` is not scalar"
                    )),
                ),
            }
        }
        IrExprKind::BinOp { left, right, .. } | IrExprKind::WrappingBinOp { left, right, .. } => {
            collect_expr_imports(left, tools, prompts, plan, agent_name)?;
            collect_expr_imports(right, tools, prompts, plan, agent_name)
        }
        IrExprKind::UnOp { operand, .. } | IrExprKind::WrappingUnOp { operand, .. } => {
            collect_expr_imports(operand, tools, prompts, plan, agent_name)
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
        | IrExprKind::TryPropagate { inner: target } => {
            collect_expr_imports(target, tools, prompts, plan, agent_name)
        }
        IrExprKind::TryRetry { body, .. } => {
            collect_expr_imports(body, tools, prompts, plan, agent_name)
        }
        IrExprKind::List { items } => {
            for item in items {
                collect_expr_imports(item, tools, prompts, plan, agent_name)?;
            }
            Ok(())
        }
        IrExprKind::Replay {
            trace,
            arms,
            else_body,
        } => {
            collect_expr_imports(trace, tools, prompts, plan, agent_name)?;
            for arm in arms {
                collect_expr_imports(&arm.body, tools, prompts, plan, agent_name)?;
            }
            collect_expr_imports(else_body, tools, prompts, plan, agent_name)
        }
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } | IrExprKind::OptionNone => Ok(()),
    }
}

fn add_tool_import(plan: &mut HostImportPlan, tool: &IrTool) -> Result<(), WasmCodegenError> {
    if plan.tool_indices.contains_key(&tool.id) {
        return Ok(());
    }
    let import = WasmHostImport {
        kind: HostImportKind::Tool,
        source_name: tool.name.clone(),
        import_name: format!("tool.{}", tool.name),
        params: tool
            .params
            .iter()
            .map(|param| validate_import_param(&tool.name, &param.name, &param.ty))
            .collect::<Result<Vec<_>, _>>()?,
        return_ty: validate_import_return(&tool.name, &tool.return_ty)?,
    };
    let index = plan.imports.len() as u32;
    plan.tool_indices.insert(tool.id, index);
    plan.imports.push(import);
    Ok(())
}

fn add_prompt_import(plan: &mut HostImportPlan, prompt: &IrPrompt) -> Result<(), WasmCodegenError> {
    if plan.prompt_indices.contains_key(&prompt.id) {
        return Ok(());
    }
    let import = WasmHostImport {
        kind: HostImportKind::Prompt,
        source_name: prompt.name.clone(),
        import_name: format!("prompt.{}", prompt.name),
        params: prompt
            .params
            .iter()
            .map(|param| validate_import_param(&prompt.name, &param.name, &param.ty))
            .collect::<Result<Vec<_>, _>>()?,
        return_ty: validate_import_return(&prompt.name, &prompt.return_ty)?,
    };
    let index = plan.imports.len() as u32;
    plan.prompt_indices.insert(prompt.id, index);
    plan.imports.push(import);
    Ok(())
}

fn add_approval_import(
    plan: &mut HostImportPlan,
    label: &str,
    args: &[IrExpr],
) -> Result<(), WasmCodegenError> {
    if plan.approval_indices.contains_key(label) {
        return Ok(());
    }
    let import = WasmHostImport {
        kind: HostImportKind::Approval,
        source_name: label.to_string(),
        import_name: format!("approve.{label}"),
        params: args
            .iter()
            .enumerate()
            .map(|(idx, arg)| {
                validate_import_param(label, &format!("arg{}", idx + 1), &arg.ty)
            })
            .collect::<Result<Vec<_>, _>>()?,
        return_ty: Type::Bool,
    };
    let index = plan.imports.len() as u32;
    plan.approval_indices.insert(label.to_string(), index);
    plan.imports.push(import);
    Ok(())
}

fn validate_import_param(
    owner: &str,
    name: &str,
    ty: &Type,
) -> Result<(String, Type), WasmCodegenError> {
    wasm_val_type(ty).map_err(|_| {
        WasmCodegenError::unsupported(format!(
            "wasm host import `{owner}` parameter `{name}` has unsupported type `{}`",
            ty.display_name()
        ))
    })?;
    Ok((name.to_string(), ty.clone()))
}

fn validate_import_return(owner: &str, ty: &Type) -> Result<Type, WasmCodegenError> {
    wasm_result_types(ty).map_err(|_| {
        WasmCodegenError::unsupported(format!(
            "wasm host import `{owner}` returns unsupported type `{}`",
            ty.display_name()
        ))
    })?;
    Ok(ty.clone())
}

fn compile_agent(
    agent: &IrAgent,
    agent_indices: &HashMap<DefId, u32>,
    host_plan: &HostImportPlan,
) -> Result<Function, WasmCodegenError> {
    let mut locals = LocalLayout::from_agent(agent)?;
    collect_block_locals(&agent.body, &mut locals)?;
    let local_groups = locals.local_groups();
    let mut function = Function::new(local_groups);

    emit_block(&agent.body, &mut function, &locals, agent_indices, host_plan)?;
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
    host_plan: &HostImportPlan,
) -> Result<(), WasmCodegenError> {
    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let {
                local_id,
                name,
                value,
                ..
            } => {
                emit_expr(value, function, locals, agent_indices, host_plan)?;
                function.instruction(&Instruction::LocalSet(locals.index(*local_id, name)?));
            }
            IrStmt::Return { value, .. } => {
                if let Some(value) = value {
                    emit_expr(value, function, locals, agent_indices, host_plan)?;
                }
                function.instruction(&Instruction::Return);
            }
            IrStmt::Expr { expr, .. } => {
                emit_expr(expr, function, locals, agent_indices, host_plan)?;
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
                emit_expr(cond, function, locals, agent_indices, host_plan)?;
                function.instruction(&Instruction::If(BlockType::Empty));
                emit_block(then_block, function, locals, agent_indices, host_plan)?;
                if let Some(else_block) = else_block {
                    function.instruction(&Instruction::Else);
                    emit_block(else_block, function, locals, agent_indices, host_plan)?;
                }
                function.instruction(&Instruction::End);
            }
            IrStmt::Approve { label, args, .. } => {
                for arg in args {
                    emit_expr(arg, function, locals, agent_indices, host_plan)?;
                }
                let index = host_plan.approval_indices.get(label).copied().ok_or_else(|| {
                    WasmCodegenError::unsupported(format!(
                        "wasm target could not resolve approval import `{label}`"
                    ))
                })?;
                function.instruction(&Instruction::Call(index));
                function.instruction(&Instruction::If(BlockType::Empty));
                function.instruction(&Instruction::Else);
                function.instruction(&Instruction::Unreachable);
                function.instruction(&Instruction::End);
            }
            IrStmt::Pass { .. } | IrStmt::Dup { .. } | IrStmt::Drop { .. } => {}
            IrStmt::Yield { .. }
            | IrStmt::For { .. }
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
    host_plan: &HostImportPlan,
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
                    emit_expr(arg, function, locals, agent_indices, host_plan)?;
                }
                let index = agent_indices.get(def_id).copied().ok_or_else(|| {
                    WasmCodegenError::unsupported(format!(
                        "wasm target could not resolve agent call `{callee_name}`"
                    ))
                })?;
                function.instruction(&Instruction::Call(index));
            }
            IrCallKind::Tool { def_id, .. } => {
                for arg in args {
                    emit_expr(arg, function, locals, agent_indices, host_plan)?;
                }
                let index = host_plan.tool_indices.get(def_id).copied().ok_or_else(|| {
                    WasmCodegenError::unsupported(format!(
                        "wasm target could not resolve tool import `{callee_name}`"
                    ))
                })?;
                function.instruction(&Instruction::Call(index));
            }
            IrCallKind::Prompt { def_id } => {
                for arg in args {
                    emit_expr(arg, function, locals, agent_indices, host_plan)?;
                }
                let index = host_plan.prompt_indices.get(def_id).copied().ok_or_else(|| {
                    WasmCodegenError::unsupported(format!(
                        "wasm target could not resolve prompt import `{callee_name}`"
                    ))
                })?;
                function.instruction(&Instruction::Call(index));
            }
            IrCallKind::StructConstructor { .. } | IrCallKind::Unknown => {
                return Err(WasmCodegenError::unsupported(format!(
                    "wasm target cannot lower non-scalar call `{callee_name}`"
                )));
            }
        },
        IrExprKind::BinOp { op, left, right } | IrExprKind::WrappingBinOp { op, left, right } => {
            emit_expr(left, function, locals, agent_indices, host_plan)?;
            emit_expr(right, function, locals, agent_indices, host_plan)?;
            emit_binary(*op, &left.ty, function)?;
        }
        IrExprKind::UnOp { op, operand } | IrExprKind::WrappingUnOp { op, operand } => {
            emit_unary(*op, operand, function, locals, agent_indices, host_plan)?;
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
    host_plan: &HostImportPlan,
) -> Result<(), WasmCodegenError> {
    match (op, &operand.ty) {
        (UnaryOp::Neg, Type::Int) => {
            function.instruction(&Instruction::I64Const(0));
            emit_expr(operand, function, locals, agent_indices, host_plan)?;
            function.instruction(&Instruction::I64Sub);
        }
        (UnaryOp::Neg, Type::Float) => {
            emit_expr(operand, function, locals, agent_indices, host_plan)?;
            function.instruction(&Instruction::F64Neg);
        }
        (UnaryOp::Not, Type::Bool) => {
            emit_expr(operand, function, locals, agent_indices, host_plan)?;
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
    fn emits_prompt_as_typed_host_import() {
        let ir = lower_src(
            r#"
prompt answer() -> Int:
    """Return 42."""

agent main() -> Int:
    return answer()
"#,
        );
        let artifacts = emit_wasm_artifacts(&ir, "prompted").expect("wasm artifacts");
        wasmparser::Validator::new()
            .validate_all(&artifacts.wasm)
            .expect("valid wasm");
        assert!(artifacts.js_loader.contains("'prompt.answer'"));
        assert!(artifacts.ts_types.contains("'answer': () => bigint"));
        assert!(artifacts.manifest_json.contains("\"kind\": \"prompt\""));
    }

    #[test]
    fn emits_tool_and_approval_as_typed_host_imports() {
        let ir = lower_src(
            r#"
tool issue_refund(amount: Int) -> Int dangerous

agent refund(amount: Int) -> Int:
    approve IssueRefund(amount)
    return issue_refund(amount)
"#,
        );
        let artifacts = emit_wasm_artifacts(&ir, "refund").expect("wasm artifacts");
        wasmparser::Validator::new()
            .validate_all(&artifacts.wasm)
            .expect("valid wasm");
        assert!(artifacts.js_loader.contains("'approve.IssueRefund'"));
        assert!(artifacts.js_loader.contains("'tool.issue_refund'"));
        assert!(artifacts.ts_types.contains("'IssueRefund': (arg1: bigint) => boolean"));
        assert!(artifacts.ts_types.contains("'issue_refund': (amount: bigint) => bigint"));
    }
}
