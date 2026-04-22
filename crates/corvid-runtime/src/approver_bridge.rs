use corvid_abi::{emit_abi, AbiAgent, EmitOptions, TypeDescription};
use corvid_ast::{AgentDecl, BinaryOp, Decl, Expr, ExternAbi, File, Literal, Stmt, UnaryOp};
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck_with_config, CorvidConfig, EffectRegistry};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const APPROVER_AGENT_NAME: &str = "approve_site";
const APPROVER_PRELUDE: &str = r#"
type ApprovalSite:
    label: String
    agent_context: String
    declared_at_file: String
    declared_at_line: Int

type ApprovalArgs:
    values: List<String>

type ApprovalContext:
    trace_run_id: String
    budget_remaining_usd: Float

type ApprovalDecision:
    accepted: Bool
    rationale: String
"#;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorvidApproverLoadStatus {
    Ok = 0,
    IoError = 1,
    CompileError = 2,
    MissingAgent = 3,
    BadSignature = 4,
    Unsafe = 5,
    OverBudget = 6,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorvidPredicateStatus {
    Ok = 0,
    BadArgs = 1,
    SiteNotFound = 2,
    Unevaluable = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CorvidPredicateResult {
    pub status: CorvidPredicateStatus,
    pub requires_approval: u8,
    pub bad_args_message: *const std::ffi::c_char,
}

#[derive(Debug, Clone)]
pub struct ApproverLoadError {
    pub status: CorvidApproverLoadStatus,
    pub message: String,
}

impl std::fmt::Display for ApproverLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApproverLoadError {}

#[derive(Debug, Clone)]
pub struct ApprovalSiteInput {
    pub site_name: String,
    pub agent_context: String,
    pub declared_at_file: String,
    pub declared_at_line: i64,
    pub budget_remaining_usd: f64,
    pub trace_run_id: String,
}

impl ApprovalSiteInput {
    pub fn fallback(label: &str) -> Self {
        Self {
            site_name: label.to_string(),
            agent_context: String::new(),
            declared_at_file: String::new(),
            declared_at_line: 0,
            budget_remaining_usd: f64::NAN,
            trace_run_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalDecisionInfo {
    pub accepted: bool,
    pub decider: String,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulatedApproverDecision {
    pub accepted: bool,
    pub rationale: String,
}

#[derive(Debug, Clone)]
struct RegisteredApprover {
    source_path: PathBuf,
    abi: AbiAgent,
    program: MiniApproverProgram,
}

fn state() -> &'static Mutex<Option<RegisteredApprover>> {
    static STATE: OnceLock<Mutex<Option<RegisteredApprover>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

pub fn register_approver_from_source(
    source_path: &Path,
    max_budget_usd_per_call: f64,
) -> Result<(), ApproverLoadError> {
    let compiled = compile_approver_source(source_path)?;
    validate_approver_safety(&compiled.abi, max_budget_usd_per_call)?;
    *state().lock().unwrap() = Some(compiled);
    Ok(())
}

pub fn clear_registered_approver() {
    *state().lock().unwrap() = None;
}

pub fn evaluate_registered_approver(
    site: &ApprovalSiteInput,
    args: &[Value],
) -> Result<Option<ApprovalDecisionInfo>, String> {
    let approver = state().lock().unwrap().clone();
    let Some(approver) = approver else {
        return Ok(None);
    };
    let decision = approver.program.evaluate(site, args)?;
    Ok(Some(ApprovalDecisionInfo {
        accepted: decision.accepted,
        decider: format!(
            "corvid-agent:{}",
            approver.source_path.to_string_lossy().replace('\\', "/")
        ),
        rationale: decision.rationale,
    }))
}

pub fn simulate_approver_source(
    source_path: &Path,
    site_label: &str,
    args_json: &str,
    max_budget_usd_per_call: f64,
) -> Result<SimulatedApproverDecision, ApproverLoadError> {
    let compiled = compile_approver_source(source_path)?;
    validate_approver_safety(&compiled.abi, max_budget_usd_per_call)?;
    let args = parse_args_array(args_json).map_err(|message| ApproverLoadError {
        status: CorvidApproverLoadStatus::BadSignature,
        message,
    })?;
    let outcome = compiled
        .program
        .evaluate(&ApprovalSiteInput::fallback(site_label), &args)
        .map_err(|message| ApproverLoadError {
            status: CorvidApproverLoadStatus::BadSignature,
            message,
        })?;
    Ok(SimulatedApproverDecision {
        accepted: outcome.accepted,
        rationale: outcome.rationale.unwrap_or_default(),
    })
}

fn compile_approver_source(source_path: &Path) -> Result<RegisteredApprover, ApproverLoadError> {
    let source = std::fs::read_to_string(source_path).map_err(|err| ApproverLoadError {
        status: CorvidApproverLoadStatus::IoError,
        message: format!("read approver source `{}`: {err}", source_path.display()),
    })?;
    let combined = format!("{APPROVER_PRELUDE}\n{source}");
    let tokens = lex(&combined).map_err(|errs| ApproverLoadError {
        status: CorvidApproverLoadStatus::CompileError,
        message: format!("lex approver source: {errs:?}"),
    })?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::CompileError,
            message: format!("parse approver source: {parse_errors:?}"),
        });
    }
    let resolved = resolve(&file);
    if !resolved.errors.is_empty() {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::CompileError,
            message: format!("resolve approver source: {:?}", resolved.errors),
        });
    }
    let checked = typecheck_with_config(&file, &resolved, None::<&CorvidConfig>);
    if !checked.errors.is_empty() {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::CompileError,
            message: format!("typecheck approver source: {:?}", checked.errors),
        });
    }
    let mut abi_file = file.clone();
    for decl in &mut abi_file.decls {
        if let Decl::Agent(agent) = decl {
            if agent.name.name == APPROVER_AGENT_NAME {
                agent.extern_abi = Some(ExternAbi::C);
            }
        }
    }
    let ir = lower(&abi_file, &resolved, &checked);
    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let registry = EffectRegistry::from_decls(&effect_decls);
    let abi = emit_abi(
        &abi_file,
        &resolved,
        &checked,
        &ir,
        &registry,
        &EmitOptions {
            source_path: &source_path.to_string_lossy().replace('\\', "/"),
            source_text: &combined,
            compiler_version: env!("CARGO_PKG_VERSION"),
            generated_at: "1970-01-01T00:00:00Z",
        },
    )
    .agents
    .into_iter()
    .find(|agent| agent.name == APPROVER_AGENT_NAME)
    .ok_or_else(|| ApproverLoadError {
        status: CorvidApproverLoadStatus::MissingAgent,
        message: format!("no `{APPROVER_AGENT_NAME}` agent found in approver source"),
    })?;
    verify_approver_signature(&abi)?;
    Ok(RegisteredApprover {
        source_path: source_path.to_path_buf(),
        abi,
        program: MiniApproverProgram::from_file(&file)?,
    })
}

fn verify_approver_signature(abi: &AbiAgent) -> Result<(), ApproverLoadError> {
    if abi.params.len() != 3 {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::BadSignature,
            message: format!(
                "`{APPROVER_AGENT_NAME}` must take exactly 3 params, got {}",
                abi.params.len()
            ),
        });
    }
    verify_struct(&abi.params[0].ty, "ApprovalSite", "parameter 1 `site`")?;
    verify_struct(&abi.params[1].ty, "ApprovalArgs", "parameter 2 `args`")?;
    verify_struct(&abi.params[2].ty, "ApprovalContext", "parameter 3 `ctx`")?;
    verify_struct(&abi.return_type, "ApprovalDecision", "return type")?;
    Ok(())
}

fn verify_struct(
    ty: &TypeDescription,
    expected: &str,
    where_: &str,
) -> Result<(), ApproverLoadError> {
    match ty {
        TypeDescription::Struct { name } if name == expected => Ok(()),
        other => Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::BadSignature,
            message: format!(
                "`{APPROVER_AGENT_NAME}` {where_} must be `{expected}`, got `{other:?}`"
            ),
        }),
    }
}

fn validate_approver_safety(
    abi: &AbiAgent,
    max_budget_usd_per_call: f64,
) -> Result<(), ApproverLoadError> {
    if abi.attributes.dangerous {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::Unsafe,
            message: "approver may not be `@dangerous`".to_string(),
        });
    }
    if abi
        .effects
        .trust_tier
        .as_deref()
        .map(|tier| tier != "autonomous")
        .unwrap_or(false)
    {
        return Err(ApproverLoadError {
            status: CorvidApproverLoadStatus::Unsafe,
            message: "approver trust tier must be `autonomous`".to_string(),
        });
    }
    if max_budget_usd_per_call > 0.0 {
        let budget = abi
            .budget
            .as_ref()
            .map(|budget| budget.usd_per_call)
            .or_else(|| abi.effects.cost.as_ref().map(|cost| cost.projected_usd))
            .unwrap_or(0.0);
        if budget > max_budget_usd_per_call {
            return Err(ApproverLoadError {
                status: CorvidApproverLoadStatus::OverBudget,
                message: format!(
                    "approver budget ${budget:.3} exceeds host ceiling ${max_budget_usd_per_call:.3}"
                ),
            });
        }
    }
    Ok(())
}

fn parse_args_array(args_json: &str) -> Result<Vec<Value>, String> {
    let value: Value = serde_json::from_str(args_json)
        .map_err(|err| format!("args_json must be a JSON array: {err}"))?;
    match value {
        Value::Array(values) => Ok(values),
        _ => Err("args_json must be a JSON array".to_string()),
    }
}

#[derive(Debug, Clone)]
struct MiniApproverProgram {
    body: corvid_ast::Block,
    struct_fields: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
enum MiniValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<MiniValue>),
    Struct {
        type_name: String,
        fields: BTreeMap<String, MiniValue>,
    },
    Nothing,
}

#[derive(Debug, Clone)]
enum MiniControl {
    Continue,
    Return(MiniValue),
}

impl MiniApproverProgram {
    fn from_file(file: &File) -> Result<Self, ApproverLoadError> {
        let body = file
            .decls
            .iter()
            .find_map(|decl| match decl {
                Decl::Agent(AgentDecl { name, body, .. }) if name.name == APPROVER_AGENT_NAME => {
                    Some(body.clone())
                }
                _ => None,
            })
            .ok_or_else(|| ApproverLoadError {
                status: CorvidApproverLoadStatus::MissingAgent,
                message: format!("no `{APPROVER_AGENT_NAME}` body found"),
            })?;
        let struct_fields = file
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Type(ty) => Some((
                    ty.name.name.clone(),
                    ty.fields
                        .iter()
                        .map(|field| field.name.name.clone())
                        .collect::<Vec<_>>(),
                )),
                _ => None,
            })
            .collect();
        Ok(Self { body, struct_fields })
    }

    fn evaluate(&self, site: &ApprovalSiteInput, args: &[Value]) -> Result<ApprovalDecisionInfo, String> {
        let mut env = BTreeMap::new();
        env.insert("site".to_string(), site_value(site));
        env.insert("args".to_string(), args_value(args));
        env.insert("ctx".to_string(), context_value(site));
        let value = match self.eval_block(&self.body, &mut env)? {
            MiniControl::Return(value) => value,
            MiniControl::Continue => return Err("approver body must return ApprovalDecision".to_string()),
        };
        let MiniValue::Struct { type_name, fields } = value else {
            return Err("approver must return ApprovalDecision".to_string());
        };
        if type_name != "ApprovalDecision" {
            return Err(format!("approver returned `{type_name}`, expected ApprovalDecision"));
        }
        let accepted = match fields.get("accepted") {
            Some(MiniValue::Bool(value)) => *value,
            _ => return Err("ApprovalDecision.accepted must be Bool".to_string()),
        };
        let rationale = match fields.get("rationale") {
            Some(MiniValue::String(value)) => Some(value.clone()),
            _ => return Err("ApprovalDecision.rationale must be String".to_string()),
        };
        Ok(ApprovalDecisionInfo {
            accepted,
            decider: String::new(),
            rationale,
        })
    }

    fn eval_block(
        &self,
        block: &corvid_ast::Block,
        env: &mut BTreeMap<String, MiniValue>,
    ) -> Result<MiniControl, String> {
        for stmt in &block.stmts {
            match self.eval_stmt(stmt, env)? {
                MiniControl::Continue => {}
                returned @ MiniControl::Return(_) => return Ok(returned),
            }
        }
        Ok(MiniControl::Continue)
    }

    fn eval_stmt(
        &self,
        stmt: &Stmt,
        env: &mut BTreeMap<String, MiniValue>,
    ) -> Result<MiniControl, String> {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let evaluated = self.eval_expr(value, env)?;
                env.insert(name.name.clone(), evaluated);
                Ok(MiniControl::Continue)
            }
            Stmt::Return { value, .. } => Ok(MiniControl::Return(match value {
                Some(value) => self.eval_expr(value, env)?,
                None => MiniValue::Nothing,
            })),
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                if self.eval_expr(cond, env)?.truthy() {
                    self.eval_block(then_block, env)
                } else if let Some(else_block) = else_block {
                    self.eval_block(else_block, env)
                } else {
                    Ok(MiniControl::Continue)
                }
            }
            Stmt::Expr { expr, .. } => {
                let _ = self.eval_expr(expr, env)?;
                Ok(MiniControl::Continue)
            }
            other => Err(format!("unsupported approver statement `{other:?}`")),
        }
    }

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &mut BTreeMap<String, MiniValue>,
    ) -> Result<MiniValue, String> {
        match expr {
            Expr::Literal { value, .. } => Ok(match value {
                Literal::Int(value) => MiniValue::Int(*value),
                Literal::Float(value) => MiniValue::Float(*value),
                Literal::String(value) => MiniValue::String(value.clone()),
                Literal::Bool(value) => MiniValue::Bool(*value),
                Literal::Nothing => MiniValue::Nothing,
            }),
            Expr::Ident { name, .. } => env
                .get(&name.name)
                .cloned()
                .ok_or_else(|| format!("unknown approver binding `{}`", name.name)),
            Expr::List { items, .. } => Ok(MiniValue::List(
                items
                    .iter()
                    .map(|item| self.eval_expr(item, env))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Expr::FieldAccess { target, field, .. } => match self.eval_expr(target, env)? {
                MiniValue::Struct { fields, .. } => fields
                    .get(&field.name)
                    .cloned()
                    .ok_or_else(|| format!("unknown field `{}`", field.name)),
                other => Err(format!("field access on non-struct `{other:?}`")),
            },
            Expr::Index { target, index, .. } => {
                let index = match self.eval_expr(index, env)? {
                    MiniValue::Int(value) => value as usize,
                    _ => return Err("list index must be Int".to_string()),
                };
                match self.eval_expr(target, env)? {
                    MiniValue::List(values) => values
                        .get(index)
                        .cloned()
                        .ok_or_else(|| format!("index {index} out of bounds")),
                    other => Err(format!("index access on non-list `{other:?}`")),
                }
            }
            Expr::Call { callee, args, .. } => {
                let Expr::Ident { name, .. } = callee.as_ref() else {
                    return Err("approver only supports constructor calls".to_string());
                };
                let Some(field_names) = self.struct_fields.get(&name.name) else {
                    return Err(format!("unsupported approver call `{}`", name.name));
                };
                if field_names.len() != args.len() {
                    return Err(format!(
                        "constructor `{}` expected {} args, got {}",
                        name.name,
                        field_names.len(),
                        args.len()
                    ));
                }
                let mut fields = BTreeMap::new();
                for (field_name, arg) in field_names.iter().zip(args.iter()) {
                    fields.insert(field_name.clone(), self.eval_expr(arg, env)?);
                }
                Ok(MiniValue::Struct {
                    type_name: name.name.clone(),
                    fields,
                })
            }
            Expr::BinOp { op, left, right, .. } => {
                let left = self.eval_expr(left, env)?;
                let right = self.eval_expr(right, env)?;
                self.eval_binop(*op, left, right)
            }
            Expr::UnOp { op, operand, .. } => {
                let value = self.eval_expr(operand, env)?;
                match op {
                    UnaryOp::Not => Ok(MiniValue::Bool(!value.truthy())),
                    UnaryOp::Neg => match value {
                        MiniValue::Int(value) => Ok(MiniValue::Int(-value)),
                        MiniValue::Float(value) => Ok(MiniValue::Float(-value)),
                        _ => Err("negation expects Int or Float".to_string()),
                    },
                }
            }
            other => Err(format!("unsupported approver expression `{other:?}`")),
        }
    }

    fn eval_binop(&self, op: BinaryOp, left: MiniValue, right: MiniValue) -> Result<MiniValue, String> {
        match op {
            BinaryOp::Eq => Ok(MiniValue::Bool(left == right)),
            BinaryOp::NotEq => Ok(MiniValue::Bool(left != right)),
            BinaryOp::And => Ok(MiniValue::Bool(left.truthy() && right.truthy())),
            BinaryOp::Or => Ok(MiniValue::Bool(left.truthy() || right.truthy())),
            BinaryOp::Gt | BinaryOp::GtEq | BinaryOp::Lt | BinaryOp::LtEq => match (left, right) {
                (MiniValue::Int(left), MiniValue::Int(right)) => Ok(MiniValue::Bool(match op {
                    BinaryOp::Gt => left > right,
                    BinaryOp::GtEq => left >= right,
                    BinaryOp::Lt => left < right,
                    BinaryOp::LtEq => left <= right,
                    _ => unreachable!(),
                })),
                (MiniValue::Float(left), MiniValue::Float(right)) => Ok(MiniValue::Bool(match op {
                    BinaryOp::Gt => left > right,
                    BinaryOp::GtEq => left >= right,
                    BinaryOp::Lt => left < right,
                    BinaryOp::LtEq => left <= right,
                    _ => unreachable!(),
                })),
                (MiniValue::String(left), MiniValue::String(right)) => Ok(MiniValue::Bool(match op {
                    BinaryOp::Gt => left > right,
                    BinaryOp::GtEq => left >= right,
                    BinaryOp::Lt => left < right,
                    BinaryOp::LtEq => left <= right,
                    _ => unreachable!(),
                })),
                _ => Err("comparison expects matching scalar operands".to_string()),
            },
            BinaryOp::Add => match (left, right) {
                (MiniValue::Int(left), MiniValue::Int(right)) => Ok(MiniValue::Int(left + right)),
                (MiniValue::Float(left), MiniValue::Float(right)) => Ok(MiniValue::Float(left + right)),
                (MiniValue::String(left), MiniValue::String(right)) => Ok(MiniValue::String(left + &right)),
                _ => Err("`+` expects matching Int, Float, or String operands".to_string()),
            },
            other => Err(format!("unsupported approver binary op `{other:?}`")),
        }
    }
}

impl MiniValue {
    fn truthy(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Int(value) => *value != 0,
            Self::Float(value) => *value != 0.0,
            Self::String(value) => !value.is_empty(),
            Self::List(values) => !values.is_empty(),
            Self::Struct { .. } => true,
            Self::Nothing => false,
        }
    }
}

impl PartialEq for MiniValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::List(left), Self::List(right)) => left == right,
            (
                Self::Struct {
                    type_name: left_ty,
                    fields: left_fields,
                },
                Self::Struct {
                    type_name: right_ty,
                    fields: right_fields,
                },
            ) => left_ty == right_ty && left_fields == right_fields,
            (Self::Nothing, Self::Nothing) => true,
            _ => false,
        }
    }
}

fn site_value(site: &ApprovalSiteInput) -> MiniValue {
    MiniValue::Struct {
        type_name: "ApprovalSite".to_string(),
        fields: BTreeMap::from([
            ("label".to_string(), MiniValue::String(site.site_name.clone())),
            (
                "agent_context".to_string(),
                MiniValue::String(site.agent_context.clone()),
            ),
            (
                "declared_at_file".to_string(),
                MiniValue::String(site.declared_at_file.clone()),
            ),
            (
                "declared_at_line".to_string(),
                MiniValue::Int(site.declared_at_line),
            ),
        ]),
    }
}

fn args_value(args: &[Value]) -> MiniValue {
    MiniValue::Struct {
        type_name: "ApprovalArgs".to_string(),
        fields: BTreeMap::from([(
            "values".to_string(),
            MiniValue::List(
                args.iter()
                    .map(|value| MiniValue::String(value.to_string()))
                    .collect(),
            ),
        )]),
    }
}

fn context_value(site: &ApprovalSiteInput) -> MiniValue {
    MiniValue::Struct {
        type_name: "ApprovalContext".to_string(),
        fields: BTreeMap::from([
            (
                "trace_run_id".to_string(),
                MiniValue::String(site.trace_run_id.clone()),
            ),
            (
                "budget_remaining_usd".to_string(),
                MiniValue::Float(site.budget_remaining_usd),
            ),
        ]),
    }
}
