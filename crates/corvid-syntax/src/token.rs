//! Token types produced by the lexer.

use corvid_ast::Span;
use serde::{Deserialize, Serialize};

/// A lexed token: kind + source location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub kind: TokKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// Every kind of token Corvid knows about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TokKind {
    // --- keywords: declarations ---
    KwAgent,
    KwTool,
    KwPrompt,
    KwEval,
    KwTest,
    KwFixture,
    KwMock,
    KwType,
    KwSession,
    KwMemory,
    KwImport,
    KwAs,
    KwPub,
    KwExtern,
    /// `extend T:` — method-attachment block.
    KwExtend,
    /// `public` — visibility modifier on methods inside
    /// `extend` blocks. Default without the keyword is private
    /// (file-scoped).
    KwPublic,
    /// `package` — used inside `public(package)` to scope visibility
    /// to the declaring package once package-level visibility exists.
    KwPackage,
    KwTry,
    KwOn,
    KwError,
    KwRetry,
    KwTimes,
    KwBackoff,
    KwLinear,
    KwExponential,

    // --- keywords: effect system ---
    KwApprove,
    KwDangerous,
    KwEffect,
    KwUses,
    KwAssert,
    KwAssertSnapshot,

    // --- keywords: typed model substrate (Phase 20h) ---
    KwModel,
    /// `requires:` clause on a prompt — sets the minimum capability
    /// level the LLM dispatch must satisfy.
    KwRequires,
    /// `route:` clause on a prompt — pattern-dispatched model
    /// selection per-call.
    KwRoute,
    /// `progressive:` clause on a prompt — try cheap model first,
    /// escalate to a stronger model if output confidence falls
    /// below the declared threshold.
    KwProgressive,
    /// `below` — used inside a `progressive:` stage to declare the
    /// confidence threshold below which escalation fires.
    KwBelow,
    /// `rollout` — one-liner A/B variant dispatch on a prompt.
    /// `rollout 10% variant, else baseline` routes ~10% of calls
    /// to `variant` and the rest to `baseline`.
    KwRollout,
    /// `ensemble` — concurrent dispatch to multiple models with
    /// a deterministic vote strategy. `ensemble [m1, m2, m3] vote
    /// majority` fires all three in parallel and returns the
    /// majority's answer.
    KwEnsemble,
    /// `vote` — names the voting strategy inside an `ensemble`
    /// clause. Currently only `majority` is supported.
    KwVote,
    /// `adversarial:` — block opening a three-stage
    /// propose / challenge / adjudicate pipeline.
    KwAdversarial,
    /// `propose:` — stage 1 of an adversarial pipeline.
    KwPropose,
    /// `challenge:` — stage 2 of an adversarial pipeline.
    KwChallenge,
    /// `adjudicate:` — stage 3 of an adversarial pipeline.
    KwAdjudicate,

    // --- keywords: control flow ---
    KwIf,
    KwElse,
    KwFor,
    KwIn,
    KwReturn,
    KwYield,
    KwBreak,
    KwContinue,
    KwPass,

    // --- keywords: replay language primitive (Phase 21) ---
    /// `replay <trace>:` — opens a replay block that pattern-matches
    /// recorded trace events and dispatches to arm bodies.
    KwReplay,
    /// `when <event_pattern> -> <expr>` — one arm of a replay block.
    /// Paired with `else` for the fallback arm.
    KwWhen,

    // --- keywords: values ---
    KwTrue,
    KwFalse,
    KwNothing,

    // --- keywords: logical ---
    KwAnd,
    KwOr,
    KwNot,

    // --- punctuation ---
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    LBrace,   // {
    RBrace,   // }
    Colon,    // :
    Comma,    // ,
    Dot,      // .
    Arrow,    // ->
    Question, // ?
    At,       // @
    Apostrophe, // '
    Dollar,   // $

    // --- operators ---
    Assign,  // =
    Eq,      // ==
    NotEq,   // !=
    Lt,      // <
    LtEq,    // <=
    Gt,      // >
    GtEq,    // >=
    Plus,    // +
    Minus,   // -
    Star,    // *
    Slash,   // /
    Percent, // %

    // --- literals ---
    Ident(String),
    Int(i64),
    Float(f64),
    StringLit(String),

    // --- structural (produced by indent pass) ---
    Newline,
    Indent,
    Dedent,

    // --- end of input ---
    Eof,
}

impl TokKind {
    /// If `s` is a Corvid keyword, return the matching `TokKind`.
    pub fn keyword_from(s: &str) -> Option<TokKind> {
        Some(match s {
            "agent" => TokKind::KwAgent,
            "tool" => TokKind::KwTool,
            "prompt" => TokKind::KwPrompt,
            "eval" => TokKind::KwEval,
            "test" => TokKind::KwTest,
            "fixture" => TokKind::KwFixture,
            "mock" => TokKind::KwMock,
            "type" => TokKind::KwType,
            "session" => TokKind::KwSession,
            "memory" => TokKind::KwMemory,
            "import" => TokKind::KwImport,
            "as" => TokKind::KwAs,
            "pub" => TokKind::KwPub,
            "extern" => TokKind::KwExtern,
            "extend" => TokKind::KwExtend,
            "public" => TokKind::KwPublic,
            "package" => TokKind::KwPackage,
            "try" => TokKind::KwTry,
            "on" => TokKind::KwOn,
            "error" => TokKind::KwError,
            "retry" => TokKind::KwRetry,
            "times" => TokKind::KwTimes,
            "backoff" => TokKind::KwBackoff,
            "linear" => TokKind::KwLinear,
            "exponential" => TokKind::KwExponential,
            "approve" => TokKind::KwApprove,
            "dangerous" => TokKind::KwDangerous,
            "effect" => TokKind::KwEffect,
            "uses" => TokKind::KwUses,
            "assert" => TokKind::KwAssert,
            "assert_snapshot" => TokKind::KwAssertSnapshot,
            "model" => TokKind::KwModel,
            "requires" => TokKind::KwRequires,
            "route" => TokKind::KwRoute,
            "progressive" => TokKind::KwProgressive,
            "below" => TokKind::KwBelow,
            "rollout" => TokKind::KwRollout,
            "ensemble" => TokKind::KwEnsemble,
            "vote" => TokKind::KwVote,
            "adversarial" => TokKind::KwAdversarial,
            "propose" => TokKind::KwPropose,
            "challenge" => TokKind::KwChallenge,
            "adjudicate" => TokKind::KwAdjudicate,
            "if" => TokKind::KwIf,
            "else" => TokKind::KwElse,
            "for" => TokKind::KwFor,
            "in" => TokKind::KwIn,
            "return" => TokKind::KwReturn,
            "yield" => TokKind::KwYield,
            "break" => TokKind::KwBreak,
            "continue" => TokKind::KwContinue,
            "pass" => TokKind::KwPass,
            "replay" => TokKind::KwReplay,
            "when" => TokKind::KwWhen,
            "true" => TokKind::KwTrue,
            "false" => TokKind::KwFalse,
            "nothing" => TokKind::KwNothing,
            "and" => TokKind::KwAnd,
            "or" => TokKind::KwOr,
            "not" => TokKind::KwNot,
            _ => return None,
        })
    }

    /// Is this a structural token emitted by the indent pass?
    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            TokKind::Newline | TokKind::Indent | TokKind::Dedent | TokKind::Eof
        )
    }
}
