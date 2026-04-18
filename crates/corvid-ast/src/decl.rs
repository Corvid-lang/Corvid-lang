//! Top-level declarations — what appears at the root of a `.cor` file.

use crate::effect::{BackpressurePolicy, Effect, EffectConstraint, EffectDecl, EffectRow};
use crate::expr::{BinaryOp, Expr};
use crate::span::{Ident, Span};
use crate::stmt::Block;
use crate::ty::{Field, Param, TypeRef};
use serde::{Deserialize, Serialize};

/// A full `.cor` source file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct File {
    pub decls: Vec<Decl>,
    pub span: Span,
}

/// Any top-level declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Decl {
    Import(ImportDecl),
    Type(TypeDecl),
    Tool(ToolDecl),
    Prompt(PromptDecl),
    Agent(AgentDecl),
    Eval(EvalDecl),
    /// `extend T:` block attaching methods to a user type.
    Extend(ExtendDecl),
    /// `effect Name:` dimensional effect declaration.
    Effect(EffectDecl),
    /// `model Name:` typed-model-substrate declaration (Phase 20h).
    /// A catalog entry for an LLM the project can dispatch to.
    Model(ModelDecl),
}

impl Decl {
    pub fn span(&self) -> Span {
        match self {
            Decl::Import(d) => d.span,
            Decl::Type(d) => d.span,
            Decl::Tool(d) => d.span,
            Decl::Prompt(d) => d.span,
            Decl::Agent(d) => d.span,
            Decl::Eval(d) => d.span,
            Decl::Extend(d) => d.span,
            Decl::Effect(d) => d.span,
            Decl::Model(d) => d.span,
        }
    }
}

/// Visibility modifier on a method declared inside an `extend` block.
/// Defaults to `Private` (file-scoped). `Public` is callable anywhere
/// the type is visible, and `PublicPackage` is reserved for a future
/// package-level visibility boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    /// Default. Callable only from the file declaring the extend block.
    Private,
    /// `public` — callable from anywhere the type is visible.
    Public,
    /// `public(package)` — callable within the declaring package once
    /// package-level visibility is wired up.
    PublicPackage,
}

impl Visibility {
    pub fn is_callable_from_outside_file(&self) -> bool {
        !matches!(self, Visibility::Private)
    }
}

/// `extend T:` block. Attaches methods to an existing
/// user-declared type. The inner decls can be any of tool / prompt /
/// agent — the receiver is the first parameter of each, whose type
/// must match the extended type. The block's visibility modifiers
/// travel with each inner decl via the parallel `visibilities` vec
/// (kept parallel rather than embedded so the existing `ToolDecl` /
/// `PromptDecl` / `AgentDecl` structs don't need new fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtendDecl {
    /// Name of the type being extended.
    pub type_name: Ident,
    /// Methods declared in the block. Each entry is an ordinary
    /// tool / prompt / agent decl (reusing existing AST structures);
    /// the surrounding `ExtendDecl` is the only thing that marks
    /// them as methods rather than free-standing declarations.
    pub methods: Vec<ExtendMethod>,
    pub span: Span,
}

/// One method inside an `extend` block. The kind-specific decl lives
/// alongside its visibility modifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtendMethod {
    pub visibility: Visibility,
    pub kind: ExtendMethodKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExtendMethodKind {
    Tool(ToolDecl),
    Prompt(PromptDecl),
    Agent(AgentDecl),
}

impl ExtendMethod {
    pub fn name(&self) -> &Ident {
        match &self.kind {
            ExtendMethodKind::Tool(d) => &d.name,
            ExtendMethodKind::Prompt(d) => &d.name,
            ExtendMethodKind::Agent(d) => &d.name,
        }
    }

    pub fn span(&self) -> Span {
        match &self.kind {
            ExtendMethodKind::Tool(d) => d.span,
            ExtendMethodKind::Prompt(d) => d.span,
            ExtendMethodKind::Agent(d) => d.span,
        }
    }

    pub fn params(&self) -> &[Param] {
        match &self.kind {
            ExtendMethodKind::Tool(d) => &d.params,
            ExtendMethodKind::Prompt(d) => &d.params,
            ExtendMethodKind::Agent(d) => &d.params,
        }
    }

    pub fn return_ty(&self) -> &TypeRef {
        match &self.kind {
            ExtendMethodKind::Tool(d) => &d.return_ty,
            ExtendMethodKind::Prompt(d) => &d.return_ty,
            ExtendMethodKind::Agent(d) => &d.return_ty,
        }
    }
}

/// Which external ecosystem an import pulls from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportSource {
    Python,
    // JavaScript, C, MCP — added in later versions.
}

/// An import statement: `import python "anthropic" as anthropic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportDecl {
    pub source: ImportSource,
    pub module: String,
    pub alias: Option<Ident>,
    pub span: Span,
}

/// A user-defined struct-like type:
///
/// ```text
/// type Ticket:
///     order_id: String
///     user_id: String
/// ```
///
/// v0.1 supports struct-like types only. Enum/union types arrive in v0.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDecl {
    pub name: Ident,
    pub fields: Vec<Field>,
    pub span: Span,
}

/// A tool declaration:
///
/// ```text
/// tool get_order(id: String) -> Order
/// tool issue_refund(id: String, amount: Float) -> Receipt dangerous
/// ```
///
/// Tools have no body — they are externally implemented and registered
/// with the runtime. The `dangerous` keyword is optional; when absent the
/// effect is `Effect::Safe`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeRef,
    pub effect: Effect,
    /// Dimensional effect row: `uses transfer_money, audit_log`.
    #[serde(default)]
    pub effect_row: EffectRow,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PromptStreamSettings {
    #[serde(default)]
    pub min_confidence: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub backpressure: Option<BackpressurePolicy>,
}

/// A prompt declaration:
///
/// ```text
/// prompt classify(t: Ticket) -> Category:
///     "Classify this ticket into one category."
/// ```
///
/// The body is a string template the compiler turns into an LLM call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeRef,
    pub template: String,
    /// Dimensional effect row: `uses llm_call, reads_context`.
    #[serde(default)]
    pub effect_row: EffectRow,
    /// `cites <param> strictly` — runtime verification that the LLM
    /// response references content from the named parameter.
    #[serde(default)]
    pub cites_strictly: Option<String>,
    /// Stream-only prompt modifiers such as `with min_confidence 0.80`.
    #[serde(default)]
    pub stream: PromptStreamSettings,
    /// `requires: <capability>` — minimum model capability this prompt
    /// needs to execute. Composed via Max through the call graph. The
    /// runtime uses this to pick the cheapest model whose `capability`
    /// field satisfies the requirement. See Phase 20h slice B.
    #[serde(default)]
    pub capability_required: Option<Ident>,
    pub span: Span,
}

/// An agent declaration:
///
/// ```text
/// agent refund_bot(ticket: Ticket) -> Decision:
///     ...
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeRef,
    pub body: Block,
    /// Declared effect row: `uses search_knowledge, transfer_money`.
    /// If empty, the typechecker infers the effect row from the body.
    #[serde(default)]
    pub effect_row: EffectRow,
    /// Constraints: `@budget($1.00)`, `@trust(autonomous)`, etc.
    #[serde(default)]
    pub constraints: Vec<EffectConstraint>,
    pub span: Span,
}

/// An eval declaration. The body executes setup code and the trailing
/// assertions validate either values or the execution trace shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalDecl {
    pub name: Ident,
    pub body: Block,
    pub assertions: Vec<EvalAssert>,
    pub span: Span,
}

/// An assertion inside an `eval` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EvalAssert {
    /// `assert <expr>` or `assert <expr> with confidence P over N runs`
    Value {
        expr: Expr,
        confidence: Option<f64>,
        runs: Option<u64>,
        span: Span,
    },
    /// `assert called <tool>`
    Called { tool: Ident, span: Span },
    /// `assert approved <label>`
    Approved { label: Ident, span: Span },
    /// `assert cost < $0.50`
    Cost {
        op: BinaryOp,
        bound: f64,
        span: Span,
    },
    /// `assert called <A> before <B>`
    Ordering {
        before: Ident,
        after: Ident,
        span: Span,
    },
}

/// `model Name:` declaration — a catalog entry for an LLM.
///
/// Each model carries a map of property name → value describing
/// cost, capability, latency, jurisdiction, privacy tier, specialty,
/// and so on. The set of valid property names is *not* hardcoded:
/// any property that corresponds to a declared dimension (built-in
/// or custom via `corvid.toml`) is accepted. This mirrors Phase 20g
/// invention #6 — the effect system is user-extensible, and the
/// model catalog extends alongside it without compiler changes.
///
/// Example:
///
/// ```text
/// model haiku:
///     cost_per_token_in: $0.00000025
///     cost_per_token_out: $0.00000125
///     capability: basic
///     latency: fast
///     max_context: 200000
///     jurisdiction: us_hosted
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDecl {
    pub name: Ident,
    pub fields: Vec<ModelField>,
    pub span: Span,
}

/// One property on a `model` block — a name and its value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelField {
    pub name: Ident,
    pub value: crate::effect::DimensionValue,
    pub span: Span,
}
