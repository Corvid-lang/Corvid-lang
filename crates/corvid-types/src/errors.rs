//! Type-checking and effect-checking error types.
//!
//! Every error carries a one-line `message` and, where possible, a `hint`
//! that tells the user exactly how to fix the problem.

use corvid_ast::Span;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub span: Span,
}

impl TypeError {
    pub fn new(kind: TypeErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// The "what went wrong" message, as a single line.
    pub fn message(&self) -> String {
        self.kind.message()
    }

    /// Optional "here's how to fix it" suggestion.
    pub fn hint(&self) -> Option<String> {
        self.kind.hint()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeWarning {
    pub kind: TypeWarningKind,
    pub span: Span,
}

impl TypeWarning {
    pub fn new(kind: TypeWarningKind, span: Span) -> Self {
        Self { kind, span }
    }

    pub fn message(&self) -> String {
        self.kind.message()
    }

    pub fn hint(&self) -> Option<String> {
        self.kind.hint()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeErrorKind {
    /// Wrong number of arguments in a call.
    ArityMismatch {
        callee: String,
        expected: usize,
        got: usize,
    },

    /// An argument's type doesn't match the parameter's declared type.
    TypeMismatch {
        expected: String,
        got: String,
        /// Optional context for where the mismatch was detected.
        context: String,
    },

    /// A field that doesn't exist on the given struct.
    UnknownField {
        struct_name: String,
        field: String,
    },

    /// Calling something that isn't callable (e.g. a primitive value).
    NotCallable { got: String },

    /// Field access on a non-struct value.
    NotAStruct { got: String },

    /// A type name used where a value was expected.
    /// E.g. `x = String` or `greet(Int)`.
    TypeAsValue { name: String },

    /// A tool or agent referenced without `()`.
    /// E.g. `x = get_order` instead of `x = get_order(id)`.
    BareFunctionReference { name: String },

    /// `expr?` was used on a value that is neither `Result` nor `Option`.
    InvalidTryPropagate { got: String },

    /// `expr?` was used in a function whose return type cannot absorb the
    /// propagated error/none branch.
    TryPropagateReturnMismatch {
        expected: String,
        got: String,
    },

    /// `try expr on error retry ...` was used on a value that is neither
    /// `Result` nor `Option`.
    InvalidRetryTarget { got: String },

    /// A compiler-known generic received the wrong number of type arguments.
    GenericArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },

    /// `Weak<T>` was declared over a non-heap-backed target type.
    InvalidWeakTargetType { got: String },

    /// `Weak::new(value)` only accepts heap-backed strong values.
    InvalidWeakNewTarget { got: String },

    /// `Weak::upgrade(value)` requires a weak reference.
    InvalidWeakUpgradeTarget { got: String },

    /// The checker cannot prove that `upgrade()` happens before an
    /// invalidating effect in the weak's effect row.
    WeakUpgradeAcrossEffects { effects: Vec<String> },

    /// The return type declared doesn't match what the body returns.
    ReturnTypeMismatch {
        expected: String,
        got: String,
    },

    /// `alias.TypeName` type reference encountered, but the Corvid
    /// `.cor` import-resolver is not yet wired up. The grammar parses
    /// cleanly; what's missing is module loading + qualified-name
    /// resolution, which lands in `lang-cor-imports-basic-resolve`.
    CorvidImportNotYetResolved {
        alias: String,
        name: String,
    },

    /// `alias.Name` references an import alias that isn't declared
    /// in the current file's `import` statements. Either a typo or
    /// a missing `import` line.
    UnknownImportAlias { alias: String },

    /// `alias.Name` references an imported file that exists, but
    /// the declaration `Name` in that file is `Private` (no
    /// `public` / `public(package)` modifier). The `lang-pub-toplevel`
    /// private-by-default rule means imports only see declarations
    /// their owner opted to expose.
    ImportedDeclIsPrivate { alias: String, name: String },

    /// `alias.Name` references a name that isn't declared in the
    /// imported file at all (publicly or privately).
    UnknownImportMember { alias: String, name: String },

    /// `yield` is only valid inside agent bodies.
    YieldOutsideAgent,

    /// An agent body used `yield` without declaring `Stream<T>`.
    YieldRequiresStreamReturn {
        declared: String,
    },

    /// A yielded value did not match the agent's declared `Stream<T>`.
    YieldReturnTypeMismatch {
        expected: String,
        got: String,
    },

    /// The headline error: a `dangerous` tool was called without a matching
    /// prior `approve` in the same block.
    UnapprovedDangerousCall {
        tool: String,
        /// The `approve` label the user should have written (PascalCase).
        expected_approve_label: String,
        arity: usize,
    },

    /// A dimensional effect constraint was violated.
    EffectConstraintViolation {
        agent: String,
        dimension: String,
        message: String,
    },

    /// `assert <expr>` inside an eval must typecheck to Bool.
    AssertNotBool { got: String },

    /// `assert called <tool>` or `assert called A before B` references
    /// a name that does not resolve to a known callable.
    EvalUnknownTool { name: String },

    /// `assert approved <label>` references an approval label that does
    /// not match any dangerous tool label in the file.
    EvalUnknownApproval { label: String },

    /// Statistical assertion modifiers must stay in range.
    InvalidConfidence { value: f64 },

    /// An agent returns `Grounded<T>` but the compiler cannot prove a
    /// provenance path from a `data: grounded` source feeds into the
    /// return value.
    UngroundedReturn {
        agent: String,
        message: String,
    },

    /// A prompt-level `cites <param> strictly` clause names a parameter
    /// that does not exist on the prompt.
    PromptCitationUnknownParam {
        prompt: String,
        param: String,
    },

    /// A prompt-level `cites <param> strictly` clause can only cite a
    /// `Grounded<T>` parameter, because runtime citation verification
    /// must be tied to a compile-time provenance-carrying context.
    PromptCitationRequiresGrounded {
        prompt: String,
        param: String,
        got: String,
    },

    /// An agent marked `@replayable` calls a function that
    /// introduces nondeterminism the trace schema cannot capture.
    /// See `crate::determinism` for the catalog of nondeterministic
    /// builtins and `docs/phase-21-determinism-sources.md` for
    /// which trace events must capture each source.
    NonReplayableCall {
        agent: String,
        call: String,
        source_label: String,
    },

    /// An agent marked `@deterministic` calls something the
    /// compiler cannot prove is pure: an LLM prompt, a tool, an
    /// approve block, a catalog-registered nondeterministic
    /// builtin, or another agent not itself marked
    /// `@deterministic`. `@deterministic` is strictly stronger
    /// than `@replayable` — it requires the agent body to be a
    /// pure function of its parameters, trace or no trace.
    NonDeterministicCall {
        agent: String,
        call: String,
        /// What the callee is: `"prompt"`, `"tool"`,
        /// `"approve"`, `"agent"` (for a non-`@deterministic`
        /// agent), or one of the catalog-source labels.
        call_kind: String,
    },

    /// A custom dimension declared in `corvid.toml` under
    /// `[effect-system.dimensions.*]` failed validation. Rejects
    /// unknown composition rules, unknown value-types, malformed
    /// defaults, and collisions with built-in dimension names.
    InvalidCustomDimension {
        dimension: String,
        message: String,
    },

    /// A `route:` arm inside a prompt points at a name that is not a
    /// `model` declaration. The runtime can only dispatch to models,
    /// so the target must be a `Decl::Model`.
    RouteTargetNotModel {
        prompt: String,
        target: String,
        got_kind: String,
    },

    /// A `route:` arm's guard expression is not a Bool.
    RouteGuardNotBool {
        prompt: String,
        got: String,
    },

    /// A `rollout N%` clause's percentage is outside `[0.0, 100.0]`.
    RolloutPercentOutOfRange {
        prompt: String,
        got: f64,
    },

    /// An `ensemble [..]` list names the same model more than once.
    /// Voting where two slots share a model degenerates to voting
    /// over fewer opinions than intended.
    EnsembleDuplicateModel {
        prompt: String,
        model: String,
    },

    /// An `adversarial:` stage (propose / challenge / adjudicate)
    /// points at a name that is not a `prompt` declaration. Stages
    /// dispatch to prompts because the runtime chains stage outputs
    /// as positional arguments to the next stage.
    AdversarialStageNotPrompt {
        prompt: String,
        stage: String,
        target: String,
        got_kind: String,
    },

    /// An `adversarial:` stage's target prompt has the wrong number
    /// of parameters for its position in the pipeline.
    AdversarialStageArity {
        prompt: String,
        stage: String,
        target: String,
        expected: usize,
        got: usize,
    },

    /// An `adversarial:` stage's target prompt has a parameter whose
    /// type does not match the previous stage's return type (or, for
    /// the proposer, the outer prompt's parameter type).
    AdversarialStageParamType {
        prompt: String,
        stage: String,
        target: String,
        index: usize,
        expected: String,
        got: String,
    },

    /// The `adjudicate` stage's return type does not match the outer
    /// prompt's declared return type.
    AdversarialStageReturnType {
        prompt: String,
        stage: String,
        target: String,
        expected: String,
        got: String,
    },

    /// The `adjudicate` stage's return type is not a struct, or the
    /// struct it returns has no `contradiction: Bool` field. The
    /// runtime reads this field to decide whether to emit
    /// `TraceEvent::AdversarialContradiction`.
    AdversarialAdjudicatorMissingContradictionField {
        prompt: String,
        target: String,
        got: String,
    },

    /// The `<expr>` in `replay <expr>:` didn't evaluate to
    /// `TraceId` (or `String`, which coerces to `TraceId` inside a
    /// replay context).
    ReplayTraceNotATraceId { got: String },

    /// A replay arm body's type doesn't match the first arm's type,
    /// so the replay expression can't have a single result type.
    /// `context` points at whether this arm is a `when` or `else`.
    ReplayArmTypeMismatch {
        expected: String,
        got: String,
        context: String,
    },

    /// `pub extern "c"` currently supports only scalar ABI types.
    NonScalarInExternC {
        agent: String,
        offender_type: String,
        position: String,
    },

    /// The compiler cannot infer a sound ownership contract for an
    /// extern-visible boundary slot without an explicit annotation.
    AmbiguousExternOwnership {
        agent: String,
        position: String,
    },

    /// A user-declared ownership annotation disagrees with the
    /// compiler's inference for the same extern-visible boundary slot.
    ExternOwnershipMismatch {
        agent: String,
        position: String,
        declared: String,
        inferred: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeWarningKind {
    /// The cost explorer could not prove a static upper bound.
    UnboundedCostAnalysis {
        agent: String,
        message: String,
    },
    /// An agent declared `Stream<T>` but never actually yielded.
    StreamReturnWithoutYield {
        agent: String,
    },
    /// A replay arm duplicates an earlier arm's pattern; the later
    /// arm can never match. Phase 21 slice 21-inv-E-3.
    ReplayUnreachableArm {
        pattern: String,
        first_arm_span: Span,
    },
}

impl TypeErrorKind {
    pub fn message(&self) -> String {
        match self {
            Self::ArityMismatch { callee, expected, got } => {
                format!(
                    "wrong number of arguments to `{callee}`: expected {expected}, got {got}"
                )
            }
            Self::TypeMismatch { expected, got, context } => {
                if context.is_empty() {
                    format!("type mismatch: expected `{expected}`, got `{got}`")
                } else {
                    format!("type mismatch in {context}: expected `{expected}`, got `{got}`")
                }
            }
            Self::UnknownField { struct_name, field } => {
                format!("no field named `{field}` on type `{struct_name}`")
            }
            Self::NotCallable { got } => {
                format!("cannot call a value of type `{got}`")
            }
            Self::NotAStruct { got } => {
                format!("field access requires a struct value, got `{got}`")
            }
            Self::TypeAsValue { name } => {
                format!("`{name}` is a type, not a value")
            }
            Self::BareFunctionReference { name } => {
                format!("`{name}` is a function; call it with `()` to use its result")
            }
            Self::InvalidTryPropagate { got } => {
                format!("`?` can only be used on `Result` or `Option`, got `{got}`")
            }
            Self::TryPropagateReturnMismatch { expected, got } => {
                format!(
                    "`?` return context mismatch: expected enclosing return type `{expected}`, got `{got}`"
                )
            }
            Self::InvalidRetryTarget { got } => {
                format!("`try ... on error retry ...` can only be used on `Result` or `Option`, got `{got}`")
            }
            Self::GenericArityMismatch { name, expected, got } => {
                format!(
                    "wrong number of type arguments for `{name}`: expected {expected}, got {got}"
                )
            }
            Self::InvalidWeakTargetType { got } => {
                format!("`Weak<T>` requires a heap-backed target type, got `{got}`")
            }
            Self::InvalidWeakNewTarget { got } => {
                format!("`Weak::new(...)` requires a heap-backed strong value, got `{got}`")
            }
            Self::InvalidWeakUpgradeTarget { got } => {
                format!("`Weak::upgrade(...)` requires a `Weak<T>` value, got `{got}`")
            }
            Self::WeakUpgradeAcrossEffects { effects } => {
                format!(
                    "`Weak::upgrade(...)` is not provably valid here: {} may have invalidated this weak since its last refresh",
                    effects.join(", ")
                )
            }
            Self::ReturnTypeMismatch { expected, got } => {
                format!(
                    "return type mismatch: declared `{expected}`, but the body returns `{got}`"
                )
            }
            Self::CorvidImportNotYetResolved { alias, name } => {
                format!(
                    "qualified type `{alias}.{name}` requires cross-file \
                     resolution, which is not yet implemented \
                     (lang-cor-imports-basic-resolve); the grammar parses \
                     cleanly but the module-loader + qualified-name lookup \
                     has not shipped"
                )
            }
            Self::UnknownImportAlias { alias } => {
                format!("no import alias `{alias}` in scope; check the `import` statements at the top of the file")
            }
            Self::ImportedDeclIsPrivate { alias, name } => {
                format!(
                    "declaration `{name}` in the module imported as `{alias}` is private; \
                     mark it with `public` in the imported file to make it importable"
                )
            }
            Self::UnknownImportMember { alias, name } => {
                format!("the module imported as `{alias}` has no declaration named `{name}`")
            }
            Self::YieldOutsideAgent => "`yield` is only allowed inside agent bodies".into(),
            Self::YieldRequiresStreamReturn { declared } => {
                format!("`yield` requires the enclosing agent to declare `Stream<T>`, got `{declared}`")
            }
            Self::YieldReturnTypeMismatch { expected, got } => {
                format!("yield type mismatch: expected `{expected}`, got `{got}`")
            }
            Self::UnapprovedDangerousCall { tool, .. } => {
                format!("dangerous tool `{tool}` called without a prior `approve`")
            }
            Self::EffectConstraintViolation { agent, message, .. } => {
                format!("effect constraint violated in agent `{agent}`: {message}")
            }
            Self::AssertNotBool { got } => {
                format!("eval assertions must be `Bool`, got `{got}`")
            }
            Self::EvalUnknownTool { name } => {
                format!("eval trace assertion references unknown callable `{name}`")
            }
            Self::EvalUnknownApproval { label } => {
                format!("eval trace assertion references unknown approval label `{label}`")
            }
            Self::InvalidConfidence { value } => {
                format!("statistical assertion confidence must be in [0.0, 1.0], got `{value}`")
            }
            Self::UngroundedReturn { agent, message } => {
                format!("ungrounded return in agent `{agent}`: {message}")
            }
            Self::PromptCitationUnknownParam { prompt, param } => {
                format!(
                    "prompt `{prompt}` cites unknown parameter `{param}` in `cites {param} strictly`"
                )
            }
            Self::PromptCitationRequiresGrounded { prompt, param, got } => {
                format!(
                    "prompt `{prompt}` cites `{param}` strictly, but `{param}` has type `{got}` instead of `Grounded<T>`"
                )
            }
            Self::NonReplayableCall {
                agent,
                call,
                source_label,
            } => {
                format!(
                    "agent `{agent}` is marked `@replayable` but calls `{call}`, which introduces a {source_label} that the trace schema cannot capture"
                )
            }
            Self::NonDeterministicCall {
                agent,
                call,
                call_kind,
            } => {
                format!(
                    "agent `{agent}` is marked `@deterministic` but calls `{call}` ({call_kind}), which the compiler cannot prove is a pure function of the agent's inputs"
                )
            }
            Self::InvalidCustomDimension { dimension, message } => {
                format!("invalid custom dimension `{dimension}` in corvid.toml: {message}")
            }
            Self::RouteTargetNotModel { prompt, target, got_kind } => {
                format!(
                    "route arm in prompt `{prompt}` points at `{target}`, which is a {got_kind}, not a `model`"
                )
            }
            Self::RouteGuardNotBool { prompt, got } => {
                format!("route arm guard in prompt `{prompt}` must evaluate to `Bool`, got `{got}`")
            }
            Self::RolloutPercentOutOfRange { prompt, got } => {
                format!("rollout percentage on prompt `{prompt}` must be in [0.0, 100.0], got `{got}`")
            }
            Self::EnsembleDuplicateModel { prompt, model } => {
                format!("ensemble on prompt `{prompt}` lists model `{model}` more than once")
            }
            Self::AdversarialStageNotPrompt { prompt, stage, target, got_kind } => {
                format!(
                    "adversarial `{stage}` stage in prompt `{prompt}` points at `{target}`, which is a {got_kind}, not a `prompt`"
                )
            }
            Self::AdversarialStageArity { prompt, stage, target, expected, got } => {
                format!(
                    "adversarial `{stage}` stage `{target}` in prompt `{prompt}` takes {got} parameter{}; expected {expected}",
                    if *got == 1 { "" } else { "s" }
                )
            }
            Self::AdversarialStageParamType { prompt, stage, target, index, expected, got } => {
                format!(
                    "adversarial `{stage}` stage `{target}` in prompt `{prompt}` parameter #{index} has type `{got}`, expected `{expected}`"
                )
            }
            Self::AdversarialStageReturnType { prompt, stage, target, expected, got } => {
                format!(
                    "adversarial `{stage}` stage `{target}` in prompt `{prompt}` returns `{got}`, expected `{expected}` to match the outer prompt's return type"
                )
            }
            Self::AdversarialAdjudicatorMissingContradictionField { prompt, target, got } => {
                format!(
                    "adversarial `adjudicate` stage `{target}` in prompt `{prompt}` returns `{got}`, which is not a struct with a `contradiction: Bool` field"
                )
            }
            Self::ReplayTraceNotATraceId { got } => {
                format!(
                    "`replay <expr>:` expects `TraceId` (or a `String` path literal), got `{got}`"
                )
            }
            Self::ReplayArmTypeMismatch { expected, got, context } => {
                format!(
                    "replay arm type mismatch in {context}: expected `{expected}` (matching the first arm), got `{got}`"
                )
            }
            Self::NonScalarInExternC {
                agent,
                offender_type,
                position,
            } => {
                format!(
                    "extern \"c\" agent `{agent}` uses unsupported ABI type `{offender_type}` in {position}"
                )
            }
            Self::AmbiguousExternOwnership { agent, position } => {
                format!(
                    "extern \"c\" agent `{agent}` has ambiguous ownership in {position}; annotate it explicitly"
                )
            }
            Self::ExternOwnershipMismatch {
                agent,
                position,
                declared,
                inferred,
                reason,
            } => {
                format!(
                    "extern \"c\" agent `{agent}` declares {declared} in {position}, but the compiler inferred {inferred}: {reason}"
                )
            }
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Self::ArityMismatch { callee, expected, .. } => Some(format!(
                "`{callee}` takes {expected} argument{}",
                if *expected == 1 { "" } else { "s" }
            )),
            Self::TypeMismatch { expected, .. } => Some(format!(
                "change the value to produce a `{expected}`, or update the signature"
            )),
            Self::UnknownField { struct_name, field } => Some(format!(
                "check the declaration of `{struct_name}` for the correct field name (you wrote `{field}`)"
            )),
            Self::NotCallable { .. } => Some(
                "only tools, agents, prompts, and imported functions can be called".into(),
            ),
            Self::NotAStruct { .. } => {
                Some("use `.field` only on values of a user-declared `type`".into())
            }
            Self::TypeAsValue { name } => Some(format!(
                "to create a value of type `{name}`, call a tool or prompt that returns one"
            )),
            Self::BareFunctionReference { name } => {
                Some(format!("did you mean `{name}(...)` ?"))
            }
            Self::InvalidTryPropagate { .. } => Some(
                "apply `?` only to `Result<T, E>` or `Option<T>` values".into(),
            ),
            Self::TryPropagateReturnMismatch { expected, .. } => Some(format!(
                "change the enclosing return type to `{expected}`, or remove `?`"
            )),
            Self::InvalidRetryTarget { .. } => Some(
                "apply retry only to `Result<T, E>` or `Option<T>` expressions".into(),
            ),
            Self::GenericArityMismatch { name, expected, .. } => Some(format!(
                "`{name}` requires {expected} type argument{}",
                if *expected == 1 { "" } else { "s" }
            )),
            Self::InvalidWeakTargetType { .. } => Some(
                "use `Weak<T>` only with heap-backed types like String, user-declared types, or List<T>".into(),
            ),
            Self::InvalidWeakNewTarget { .. } => Some(
                "pass a String, user-declared type, or List<T> value to `Weak::new(...)`".into(),
            ),
            Self::InvalidWeakUpgradeTarget { .. } => Some(
                "call `Weak::upgrade(...)` only on a value whose type is `Weak<T>`".into(),
            ),
            Self::WeakUpgradeAcrossEffects { effects } => Some(format!(
                "refresh the weak with a new `Weak::new(...)` or an earlier `Weak::upgrade(...)`, and avoid `{}` on every path before this call",
                effects.join(", ")
            )),
            Self::ReturnTypeMismatch { expected, .. } => Some(format!(
                "change the final `return` to produce a `{expected}`, or update the declared return type"
            )),
            Self::CorvidImportNotYetResolved { .. } => Some(
                "for now, use an unqualified local type or wait for cross-file Corvid import resolution to land"
                    .into(),
            ),
            Self::UnknownImportAlias { alias } => Some(format!(
                "add `import \"./path\" as {alias}` at the top of the file (or fix the alias spelling)"
            )),
            Self::ImportedDeclIsPrivate { name, .. } => Some(format!(
                "in the imported file, add `public` before `{name}`'s declaration to export it"
            )),
            Self::UnknownImportMember { .. } => Some(
                "check the imported file's top-level declarations for the name you meant".into(),
            ),
            Self::YieldOutsideAgent => Some(
                "move `yield` into an `agent ... -> Stream<T>` body, or replace it with `return`".into(),
            ),
            Self::YieldRequiresStreamReturn { .. } => Some(
                "change the agent return type to `Stream<T>` matching the yielded value type".into(),
            ),
            Self::YieldReturnTypeMismatch { .. } => Some(
                "yield values whose type matches the declared `Stream<T>` element type".into(),
            ),
            Self::UnapprovedDangerousCall {
                expected_approve_label,
                arity,
                ..
            } => {
                let args = (0..*arity)
                    .map(|i| format!("arg{}", i + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "add `approve {expected_approve_label}({args})` on the line before this call"
                ))
            }
            Self::EffectConstraintViolation { dimension, .. } => Some(format!(
                "relax the `@{dimension}` constraint, or remove the call that violates it"
            )),
            Self::AssertNotBool { .. } => Some(
                "make the asserted expression evaluate to `Bool`, for example by adding a comparison".into(),
            ),
            Self::EvalUnknownTool { .. } => Some(
                "declare the referenced tool, prompt, or agent before using it in `assert called ...`".into(),
            ),
            Self::EvalUnknownApproval { .. } => Some(
                "use the PascalCase approval label for a dangerous tool declared in this file".into(),
            ),
            Self::InvalidConfidence { .. } => Some(
                "use `with confidence P over N runs` with 0.0 <= P <= 1.0 and N > 0".into(),
            ),
            Self::UngroundedReturn { .. } => Some(
                "call a tool declared `uses retrieval` (or any effect with `data: grounded`) \
                 and pass its result to the return value, directly or through a prompt"
                    .into(),
            ),
            Self::PromptCitationUnknownParam { .. } => Some(
                "cite one of the prompt parameters, for example `cites ctx strictly`".into(),
            ),
            Self::PromptCitationRequiresGrounded { .. } => Some(
                "make the cited parameter `Grounded<T>` or pass a retrieval-backed value into the prompt"
                    .into(),
            ),
            Self::NonReplayableCall { call, .. } => Some(format!(
                "route `{call}` through a recorded interface (a tool, prompt, or captured builtin) \
                 so replay can substitute the recorded value, or drop the `@replayable` attribute"
            )),
            Self::NonDeterministicCall { call, call_kind, .. } => Some(format!(
                "`@deterministic` requires a pure function of the agent's inputs. Either remove \
                 the call to `{call}` ({call_kind}), mark the callee `@deterministic` if it is \
                 an agent you control, or drop the `@deterministic` attribute and use \
                 `@replayable` if replay reproducibility is enough"
            )),
            Self::InvalidCustomDimension { .. } => Some(
                "see docs/effects-spec/01-dimensional-syntax.md §4 for the supported \
                 composition rules, value types, and default-value shapes"
                    .into(),
            ),
            Self::RouteTargetNotModel { target, .. } => Some(format!(
                "declare `{target}` as a `model ...:` block, or route to an existing model"
            )),
            Self::RouteGuardNotBool { .. } => Some(
                "use a comparison or boolean expression for the guard, e.g. `length(q) > 1000`"
                    .into(),
            ),
            Self::RolloutPercentOutOfRange { .. } => Some(
                "use a percentage between 0 and 100, e.g. `rollout 10% new_v2, else old_v1`"
                    .into(),
            ),
            Self::EnsembleDuplicateModel { .. } => Some(
                "list each ensemble model at most once; dispatch to distinct providers to get independent votes"
                    .into(),
            ),
            Self::AdversarialStageNotPrompt { target, .. } => Some(format!(
                "declare `{target}` as a `prompt ...` with the right signature, or point the stage at an existing prompt"
            )),
            Self::AdversarialStageArity { stage, expected, .. } => Some(match stage.as_str() {
                "propose" => format!(
                    "the `propose` stage runs first; it must accept the same {expected} parameter(s) as the outer prompt"
                ),
                "challenge" => "the `challenge` stage must accept exactly 1 parameter: the proposer's return value".into(),
                "adjudicate" => "the `adjudicate` stage must accept exactly 2 parameters: the proposer's return value followed by the challenger's return value".into(),
                _ => format!("change the stage's arity to {expected}"),
            }),
            Self::AdversarialStageParamType { stage, index, expected, .. } => Some(match stage.as_str() {
                "propose" => format!(
                    "parameter #{index} must match the outer prompt's parameter at the same position; expected `{expected}`"
                ),
                "challenge" => format!(
                    "the `challenge` stage's parameter must accept the proposer's return type `{expected}`"
                ),
                "adjudicate" if *index == 0 => format!(
                    "the `adjudicate` stage's first parameter must accept the proposer's return type `{expected}`"
                ),
                "adjudicate" => format!(
                    "the `adjudicate` stage's second parameter must accept the challenger's return type `{expected}`"
                ),
                _ => format!("change the parameter type to `{expected}`"),
            }),
            Self::AdversarialStageReturnType { expected, .. } => Some(format!(
                "change the `adjudicate` stage's return type to `{expected}`, or update the outer prompt's return type to match"
            )),
            Self::AdversarialAdjudicatorMissingContradictionField { .. } => Some(
                "declare a `type` with at least `contradiction: Bool` and return it from the adjudicator — the runtime reads this field to decide whether to emit an adversarial contradiction trace event"
                    .into(),
            ),
            Self::ReplayTraceNotATraceId { .. } => Some(
                "pass either a `String` path literal like `\"run.jsonl\"` or a value of type `TraceId`"
                    .into(),
            ),
            Self::ReplayArmTypeMismatch { expected, .. } => Some(format!(
                "every arm (including `else`) of a replay block must produce the same type; adjust the arm body to return `{expected}`"
            )),
            Self::NonScalarInExternC { .. } => Some(
                "extern \"c\" currently accepts Int/Float/Bool/String parameters plus scalar, `Grounded<scalar>`, or `Nothing` returns; rich structured boundary types still wait for later Phase 22 FFI slices".into(),
            ),
            Self::AmbiguousExternOwnership { .. } => Some(
                "add an explicit ownership annotation such as `@owned`, `@borrowed`, `@shared`, or `@static` after the boundary type".into(),
            ),
            Self::ExternOwnershipMismatch { inferred, .. } => Some(format!(
                "either change the annotation to `{inferred}` or change the implementation so the inferred ownership matches the declared contract"
            )),
        }
    }
}

impl TypeWarningKind {
    pub fn message(&self) -> String {
        match self {
            Self::UnboundedCostAnalysis { agent, message } => {
                format!("cost analysis warning in agent `{agent}`: {message}")
            }
            Self::StreamReturnWithoutYield { agent } => {
                format!("W0270: agent `{agent}` declares `Stream<T>` return but never yields")
            }
            Self::ReplayUnreachableArm { pattern, first_arm_span } => {
                format!(
                    "replay arm `{pattern}` is unreachable: an earlier arm at [{}..{}] already matches the same recorded events",
                    first_arm_span.start, first_arm_span.end
                )
            }
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Self::UnboundedCostAnalysis { .. } => Some(
                "use a statically bounded loop or inspect `:cost <agent>` for the partial tree".into(),
            ),
            Self::StreamReturnWithoutYield { .. } => Some(
                "either add at least one `yield` or change the return type to a non-stream value".into(),
            ),
            Self::ReplayUnreachableArm { .. } => Some(
                "remove the duplicate arm or make its pattern distinct (different prompt / tool / label)".into(),
            ),
        }
    }
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.message())?;
        if let Some(hint) = self.hint() {
            write!(f, "\n  help: {hint}")?;
        }
        Ok(())
    }
}

impl fmt::Display for TypeWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.message())?;
        if let Some(hint) = self.hint() {
            write!(f, "\n  help: {hint}")?;
        }
        Ok(())
    }
}
