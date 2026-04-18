//! IR node types.
//!
//! A flatter, normalized form of the typed AST. References are already
//! resolved to `DefId`/`LocalId`; every expression carries its `Type`.

use corvid_ast::{Backoff, BackpressurePolicy, BinaryOp, Effect, Span, UnaryOp};
use corvid_resolve::{DefId, LocalId};
use corvid_types::Type;

/// A full `.cor` file in IR form.
#[derive(Debug, Clone)]
pub struct IrFile {
    pub imports: Vec<IrImport>,
    pub types: Vec<IrType>,
    pub tools: Vec<IrTool>,
    pub prompts: Vec<IrPrompt>,
    pub agents: Vec<IrAgent>,
    pub evals: Vec<IrEval>,
}

/// `import python "..." as alias`.
#[derive(Debug, Clone)]
pub struct IrImport {
    pub id: DefId,
    pub source: IrImportSource,
    pub module: String,
    pub alias: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrImportSource {
    Python,
}

/// A user-declared struct.
#[derive(Debug, Clone)]
pub struct IrType {
    pub id: DefId,
    pub name: String,
    pub fields: Vec<IrField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IrField {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

/// A tool declaration (no body — externally implemented).
#[derive(Debug, Clone)]
pub struct IrTool {
    pub id: DefId,
    pub name: String,
    pub params: Vec<IrParam>,
    pub return_ty: Type,
    pub effect: Effect,
    pub effect_names: Vec<String>,
    /// If any declared effect has `trust: autonomous_if_confident(T)`,
    /// this carries the confidence threshold. At runtime, the
    /// interpreter checks composed input confidence and activates the
    /// approval gate if confidence is below this threshold.
    pub confidence_gate: Option<f64>,
    pub span: Span,
}

/// A prompt declaration (body is a template string).
#[derive(Debug, Clone)]
pub struct IrPrompt {
    pub id: DefId,
    pub name: String,
    pub params: Vec<IrParam>,
    pub return_ty: Type,
    pub template: String,
    pub effect_names: Vec<String>,
    pub effect_cost: f64,
    pub effect_confidence: f64,
    /// Index of the parameter whose content must appear in the LLM response.
    /// Set when the prompt declares `cites <param> strictly`.
    pub cites_strictly_param: Option<usize>,
    /// Stream-only prompt modifiers preserved for the interpreter tier.
    pub min_confidence: Option<f64>,
    pub max_tokens: Option<u64>,
    pub backpressure: Option<BackpressurePolicy>,
    /// Phase 20h: minimum model capability this prompt requires
    /// (`basic` | `standard` | `expert` | custom). The runtime
    /// uses this to pick the cheapest declared model whose own
    /// `capability` field satisfies the requirement. `None` means
    /// the prompt uses the default-capability model (first in the
    /// catalog, or the `default_model`-backed pipeline that shipped
    /// before the model substrate existed).
    pub capability_required: Option<String>,
    /// Phase 20h slice C: pattern-dispatched per-call model
    /// selection. Empty `arms` means the prompt uses the standard
    /// capability-based dispatch (slice B). Non-empty means the
    /// runtime evaluates each arm's guard in order and dispatches
    /// to the first match's model.
    pub route: Vec<IrRouteArm>,
    pub span: Span,
}

/// One arm of a prompt's `route:` clause at IR level.
#[derive(Debug, Clone)]
pub struct IrRouteArm {
    pub pattern: IrRoutePattern,
    /// DefId of the target `model` declaration.
    pub model_def_id: DefId,
    pub model_name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrRoutePattern {
    Wildcard,
    Guard(IrExpr),
}

/// An agent declaration with a typed body.
#[derive(Debug, Clone)]
pub struct IrAgent {
    pub id: DefId,
    pub name: String,
    pub params: Vec<IrParam>,
    pub return_ty: Type,
    pub cost_budget: Option<f64>,
    pub body: IrBlock,
    pub span: Span,
    /// Per-parameter ownership at the callee ABI.
    /// `None` = ownership analysis hasn't run on this agent (every
    /// parameter is treated as Owned, matching pre-17b behavior).
    /// `Some(v)` with `v.len() == params.len()` — each entry matches
    /// the parameter at the same index.
    ///
    /// Populated by `corvid-codegen-cl`'s ownership pass after IR
    /// lowering and before Cranelift codegen. The interpreter tier
    /// (`corvid-vm`) ignores this field — refcount there is via `Arc`
    /// and has no ABI distinction.
    pub borrow_sig: Option<Vec<ParamBorrow>>,
}

/// An eval declaration lowered into IR.
#[derive(Debug, Clone)]
pub struct IrEval {
    pub id: DefId,
    pub name: String,
    pub body: IrBlock,
    pub assertions: Vec<IrEvalAssert>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrEvalAssert {
    Value {
        expr: IrExpr,
        confidence: Option<f64>,
        runs: Option<u64>,
        span: Span,
    },
    Called {
        def_id: DefId,
        name: String,
        span: Span,
    },
    Approved {
        label: String,
        span: Span,
    },
    Cost {
        op: BinaryOp,
        bound: f64,
        span: Span,
    },
    Ordering {
        before_id: DefId,
        before_name: String,
        after_id: DefId,
        after_name: String,
        span: Span,
    },
}

/// Callee-side ABI for a refcounted parameter. Non-refcounted params
/// (Int, Float, Bool) have no RC ABI decision — this enum describes
/// them as `Owned` trivially (no retain/release either way).
///
/// Defined in corvid-ir rather than corvid-codegen-cl so the
/// interpreter crate can see it (and explicitly ignore it) without a
/// cross-crate cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamBorrow {
    /// Caller transfers a +1 on the argument; callee is responsible
    /// for eventual drop. Matches pre-17b behavior for all parameters.
    Owned,
    /// Caller does not transfer a +1; callee must NOT drop and must
    /// emit `Dup` locally before storing the value into a long-lived
    /// location or returning it. Saves one retain at the caller + one
    /// release at the callee when the body is read-only on the param.
    Borrowed,
}

#[derive(Debug, Clone)]
pub struct IrParam {
    pub name: String,
    pub local_id: LocalId,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IrBlock {
    pub stmts: Vec<IrStmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrStmt {
    /// `x = expr` — binds `local_id` to the value of `value`.
    Let {
        local_id: LocalId,
        name: String,
        ty: Type,
        value: IrExpr,
        span: Span,
    },

    /// `return expr?`
    Return {
        value: Option<IrExpr>,
        span: Span,
    },

    /// `yield expr`
    Yield {
        value: IrExpr,
        span: Span,
    },

    /// `if cond: then else else_`
    If {
        cond: IrExpr,
        then_block: IrBlock,
        else_block: Option<IrBlock>,
        span: Span,
    },

    /// `for var in iter: body`
    For {
        var_local: LocalId,
        var_name: String,
        iter: IrExpr,
        body: IrBlock,
        span: Span,
    },

    /// `approve Label(args)` — authorizes matching dangerous tool calls.
    Approve {
        label: String,
        args: Vec<IrExpr>,
        span: Span,
    },

    /// Expression evaluated for side effects.
    Expr { expr: IrExpr, span: Span },

    /// `break`, `continue`, `pass` — dedicated IR variants.
    Break { span: Span },
    Continue { span: Span },
    Pass { span: Span },

    /// Increment a refcounted local's refcount.
    /// Inserted by the ownership analysis pass at non-final uses of a
    /// binding. Codegen lowers this as a single `corvid_retain` call.
    /// The interpreter ignores it (Arc handles refcount implicitly).
    ///
    /// `Dup` on a non-refcounted local is a no-op — the analysis pass
    /// emits it only for refcounted types, but the codegen double-
    /// checks via the local's declared type before emitting.
    Dup { local_id: LocalId, span: Span },

    /// Release a refcounted local's refcount.
    /// Inserted at final use (unless the use is a consume/move) or at
    /// scope exit for any still-owned bindings. Codegen lowers this as
    /// a single `corvid_release` call. The interpreter ignores it.
    Drop { local_id: LocalId, span: Span },
}

#[derive(Debug, Clone)]
pub struct IrExpr {
    pub kind: IrExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrExprKind {
    /// A literal value.
    Literal(IrLiteral),

    /// Reference to a parameter or local binding.
    Local {
        local_id: LocalId,
        name: String,
    },

    /// Reference to a top-level declaration (imports only in v0.1).
    Decl { def_id: DefId, name: String },

    /// `tool_or_agent_or_prompt(args)` — resolved to a specific declaration.
    Call {
        kind: IrCallKind,
        callee_name: String,
        args: Vec<IrExpr>,
    },

    FieldAccess {
        target: Box<IrExpr>,
        field: String,
    },

    Index {
        target: Box<IrExpr>,
        index: Box<IrExpr>,
    },

    BinOp {
        op: BinaryOp,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },

    UnOp {
        op: UnaryOp,
        operand: Box<IrExpr>,
    },

    List { items: Vec<IrExpr> },

    WeakNew { strong: Box<IrExpr> },
    WeakUpgrade { weak: Box<IrExpr> },
    ResultOk { inner: Box<IrExpr> },
    ResultErr { inner: Box<IrExpr> },
    OptionSome { inner: Box<IrExpr> },
    OptionNone,
    TryPropagate { inner: Box<IrExpr> },
    TryRetry {
        body: Box<IrExpr>,
        attempts: u64,
        backoff: Backoff,
    },
}

#[derive(Debug, Clone)]
pub enum IrLiteral {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Nothing,
}

/// What the call resolves to. Lets the codegen emit the right thing.
#[derive(Debug, Clone)]
pub enum IrCallKind {
    /// Tool call. Codegen dispatches through the runtime so effect +
    /// audit metadata can travel with the call.
    Tool { def_id: DefId, effect: Effect },
    /// Prompt call. Codegen routes through the LLM runtime.
    Prompt { def_id: DefId },
    /// Agent call — recursion or composition.
    Agent { def_id: DefId },
    /// Struct constructor — `Order(id, amount)` builds an `Order`.
    /// Args are field values in declaration order. Codegen lowers as
    /// an allocation followed by per-field stores.
    StructConstructor { def_id: DefId },
    /// Something we couldn't resolve (graceful degradation).
    Unknown,
}
