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
    pub tests: Vec<IrTest>,
    pub fixtures: Vec<IrFixture>,
    pub mocks: Vec<IrMock>,
}

/// `import python "..." as alias`.
#[derive(Debug, Clone)]
pub struct IrImport {
    pub id: DefId,
    pub source: IrImportSource,
    pub module: String,
    pub content_hash: Option<IrImportContentHash>,
    pub alias: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IrImportContentHash {
    pub algorithm: String,
    pub hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrImportSource {
    Python,
    Corvid,
    RemoteCorvid,
    PackageCorvid,
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
    pub escalate_to: Option<String>,
    /// Runtime calibration flag. When true, prompt calls record
    /// confidence-vs-accuracy observations if the adapter supplies
    /// correctness metadata.
    pub calibrated: bool,
    /// Runtime prompt-response cache opt-in. Cache identity includes
    /// selected model, rendered prompt, JSON arguments, and output schema.
    pub cacheable: bool,
    /// Phase 20h: minimum model capability this prompt requires
    /// (`basic` | `standard` | `expert` | custom). The runtime
    /// uses this to pick the cheapest declared model whose own
    /// `capability` field satisfies the requirement. `None` means
    /// the prompt uses the default-capability model (first in the
    /// catalog, or the `default_model`-backed pipeline that shipped
    /// before the model substrate existed).
    pub capability_required: Option<String>,
    /// Required model output format (`strict_json`,
    /// `markdown_strict`, etc.). Runtime selection uses this as a
    /// hard eligibility filter.
    pub output_format_required: Option<String>,
    /// Phase 20h slice C: pattern-dispatched per-call model
    /// selection. Empty `arms` means the prompt uses the standard
    /// capability-based dispatch (slice B). Non-empty means the
    /// runtime evaluates each arm's guard in order and dispatches
    /// to the first match's model.
    pub route: Vec<IrRouteArm>,
    /// Phase 20h slice E: progressive refinement chain. Empty
    /// means the prompt doesn't use progressive dispatch. Non-empty
    /// means the runtime runs stages in order; each non-final
    /// stage's `threshold` is the minimum output confidence at
    /// which to accept the stage's result. If a stage's output is
    /// below its threshold, the runtime escalates to the next
    /// stage. The final stage has `threshold = None` and always
    /// runs as the terminal fallback.
    pub progressive: Vec<IrProgressiveStage>,
    /// Phase 20h slice I: A/B rollout. `None` means no rollout
    /// is configured. `Some(spec)` routes a fraction of calls to
    /// `spec.variant_def_id` and the rest to `spec.baseline_def_id`.
    /// Runtime chooses per-call (deterministic or random — that's
    /// Dev B's C-rt cohort decision).
    pub rollout: Option<IrRolloutSpec>,
    /// Phase 20h slice F: concurrent voting across multiple models.
    /// `None` means no ensemble. `Some(spec)` means the runtime
    /// dispatches to every model in `spec.models` concurrently and
    /// applies `spec.vote` to pick the winner.
    pub ensemble: Option<IrEnsembleSpec>,
    /// Phase 20h slice G: three-stage propose / challenge /
    /// adjudicate pipeline. Runtime dispatches sequentially —
    /// adjudicator's output is the prompt's result. Prior stages'
    /// outputs are available as reserved template variables.
    pub adversarial: Option<IrAdversarialSpec>,
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

/// One stage of a prompt's `progressive:` chain at IR level.
/// `threshold = None` marks the terminal fallback (always runs).
#[derive(Debug, Clone)]
pub struct IrProgressiveStage {
    pub model_def_id: DefId,
    pub model_name: String,
    pub threshold: Option<f64>,
    pub span: Span,
}

/// Lowered A/B rollout spec.
#[derive(Debug, Clone)]
pub struct IrRolloutSpec {
    /// Percentage of calls routed to the variant (0.0 – 100.0).
    pub variant_percent: f64,
    pub variant_def_id: DefId,
    pub variant_name: String,
    pub baseline_def_id: DefId,
    pub baseline_name: String,
    pub span: Span,
}

/// Lowered ensemble voting spec.
#[derive(Debug, Clone)]
pub struct IrEnsembleSpec {
    /// Models to dispatch to concurrently. Runtime fires them via
    /// `tokio::join!` and applies the vote strategy to the results.
    pub models: Vec<IrEnsembleMember>,
    pub vote: IrVoteStrategy,
    pub weighting: Option<IrEnsembleWeighting>,
    pub disagreement_escalation: Option<IrEnsembleMember>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IrEnsembleMember {
    pub def_id: DefId,
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrVoteStrategy {
    Majority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrEnsembleWeighting {
    AccuracyHistory,
}

/// Lowered adversarial pipeline. Runtime runs proposer →
/// challenger → adjudicator; the adjudicator's output is returned.
#[derive(Debug, Clone)]
pub struct IrAdversarialSpec {
    pub proposer_def_id: DefId,
    pub proposer_name: String,
    pub challenger_def_id: DefId,
    pub challenger_name: String,
    pub adjudicator_def_id: DefId,
    pub adjudicator_name: String,
    pub span: Span,
}

/// An agent declaration with a typed body.
#[derive(Debug, Clone)]
pub struct IrAgent {
    pub id: DefId,
    pub name: String,
    pub extern_abi: Option<IrExternAbi>,
    pub params: Vec<IrParam>,
    pub return_ty: Type,
    pub cost_budget: Option<f64>,
    pub wrapping_arithmetic: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrExternAbi {
    C,
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

/// A test declaration lowered into IR. The runner lands in Phase 26 after the
/// compiler can already preserve test bodies and assertion metadata.
#[derive(Debug, Clone)]
pub struct IrTest {
    pub id: DefId,
    pub name: String,
    pub body: IrBlock,
    pub assertions: Vec<IrEvalAssert>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IrFixture {
    pub id: DefId,
    pub name: String,
    pub params: Vec<IrParam>,
    pub return_ty: Type,
    pub body: IrBlock,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IrMock {
    pub target_id: DefId,
    pub target_name: String,
    pub params: Vec<IrParam>,
    pub return_ty: Type,
    pub body: IrBlock,
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
    Snapshot {
        expr: IrExpr,
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

    /// Integer arithmetic under an enclosing `@wrapping` agent.
    /// Defaults remain checked; this node makes the opt-out explicit
    /// for every runtime/codegen tier.
    WrappingBinOp {
        op: BinaryOp,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },

    UnOp {
        op: UnaryOp,
        operand: Box<IrExpr>,
    },

    /// Integer unary operations under an enclosing `@wrapping` agent.
    WrappingUnOp {
        op: UnaryOp,
        operand: Box<IrExpr>,
    },

    List { items: Vec<IrExpr> },

    /// `grounded.unwrap_discarding_sources()` — consciously erase the
    /// provenance wrapper and keep the inner value.
    UnwrapGrounded { value: Box<IrExpr> },

    WeakNew { strong: Box<IrExpr> },
    WeakUpgrade { weak: Box<IrExpr> },
    StreamSplitBy {
        stream: Box<IrExpr>,
        key: String,
    },
    StreamMerge {
        groups: Box<IrExpr>,
        policy: StreamMergePolicy,
    },
    StreamOrderedBy {
        stream: Box<IrExpr>,
        policy: StreamMergePolicy,
    },
    StreamResumeToken { stream: Box<IrExpr> },
    ResumeStream {
        prompt_def_id: DefId,
        prompt_name: String,
        token: Box<IrExpr>,
    },
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

    /// `replay <trace>: when <pat> -> <body> else <body>` — the
    /// language-level replay primitive. Runtime semantics
    /// (21-inv-E-runtime): load the trace referenced by `trace`,
    /// walk its event stream, match each event against the arms in
    /// source order, and execute the first matching arm's body with
    /// captures bound. If no event in the trace matches any arm,
    /// execute `else_body`.
    ///
    /// Arms retain their `when` source order so runtime dispatch is
    /// unambiguous (first-match-wins). The `else_body` is separate
    /// rather than a trailing arm so codegen and the checker can
    /// both treat it as required — the grammar enforced that in
    /// 21-inv-E-1.
    Replay {
        trace: Box<IrExpr>,
        arms: Vec<IrReplayArm>,
        else_body: Box<IrExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMergePolicy {
    Fifo,
    FairRoundRobin,
    Sorted,
}

impl StreamMergePolicy {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "fifo" => Some(Self::Fifo),
            "fair_round_robin" => Some(Self::FairRoundRobin),
            "sorted" => Some(Self::Sorted),
            _ => None,
        }
    }
}

/// One lowered arm of a replay block: pattern + optional
/// whole-event capture + body. Per-arg captures (tool-arg
/// identifier captures) live inside the pattern so the runtime
/// sees them alongside the literal / wildcard arg shapes.
#[derive(Debug, Clone)]
pub struct IrReplayArm {
    pub pattern: IrReplayPattern,
    /// `Some(local)` iff the arm had an `as <ident>` tail. The
    /// local's type is already in the type side-table from the
    /// checker slice (21-inv-E-3) and will be populated with the
    /// recorded event's payload value at runtime.
    pub capture: Option<IrReplayCapture>,
    pub body: Box<IrExpr>,
    pub span: Span,
}

/// A whole-event capture's runtime handle: the `LocalId` the
/// arm body reads from, plus the declared name for diagnostics.
#[derive(Debug, Clone)]
pub struct IrReplayCapture {
    pub local_id: LocalId,
    pub name: String,
    pub span: Span,
}

/// A lowered replay pattern. The string `prompt` / `tool` /
/// `label` fields are what the runtime matches against recorded
/// events' names — trace events carry strings, not DefIds.
#[derive(Debug, Clone)]
pub enum IrReplayPattern {
    Llm { prompt: String, span: Span },
    Tool {
        tool: String,
        arg: IrReplayToolArgPattern,
        span: Span,
    },
    Approve { label: String, span: Span },
}

impl IrReplayPattern {
    pub fn span(&self) -> Span {
        match self {
            Self::Llm { span, .. }
            | Self::Tool { span, .. }
            | Self::Approve { span, .. } => *span,
        }
    }
}

/// The three shapes a tool-arg pattern can take, one-to-one with
/// the AST forms. `Capture` carries the same `IrReplayCapture`
/// handle the whole-event capture uses, so runtime binding is
/// uniform.
#[derive(Debug, Clone)]
pub enum IrReplayToolArgPattern {
    Wildcard,
    StringLit(String),
    Capture(IrReplayCapture),
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
    Fixture { def_id: DefId },
    /// Struct constructor — `Order(id, amount)` builds an `Order`.
    /// Args are field values in declaration order. Codegen lowers as
    /// an allocation followed by per-field stores.
    StructConstructor { def_id: DefId },
    /// Something we couldn't resolve (graceful degradation).
    Unknown,
}
