//! Public typed records for the guarantee registry — slice 35
//! / canonical contract surface, decomposed in Phase 20j-A8.
//!
//! Three enums + one struct exhaustively describe every public
//! Corvid guarantee:
//!
//! - [`GuaranteeKind`] — the moat dimension (approval, effect
//!   row, grounded, budget, confidence, replay, provenance, ABI,
//!   server, jobs, auth, connector, observability, platform).
//! - [`GuaranteeClass`] — `Static` / `RuntimeChecked` /
//!   `OutOfScope`. Drives the registry's honesty rules.
//! - [`Phase`] — pipeline lane that owns enforcement (Resolve,
//!   TypeCheck, IrLower, Codegen, Sign, Runtime, Platform).
//! - [`Guarantee`] — the row itself: id + kind + class + phase +
//!   description + out_of_scope_reason + per-row test refs.

use std::fmt;

/// What kind of contract a guarantee enforces.
///
/// Kinds are coarse categories matching Corvid's moat dimensions:
/// approval boundaries, effect rows, grounding, cost budgets,
/// confidence thresholds, replay determinism, provenance, and the
/// ABI surface. Every guarantee in the registry belongs to exactly
/// one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GuaranteeKind {
    Approval,
    EffectRow,
    Grounded,
    Budget,
    Confidence,
    Replay,
    ProvenanceTrace,
    AbiDescriptor,
    AbiAttestation,
    Server,
    Jobs,
    Auth,
    Connector,
    Observability,
    Platform,
}

impl GuaranteeKind {
    /// Stable lowercase slug used by JSON serialisation, doc
    /// generation, and CLI output. Slugs MUST stay stable across
    /// versions — they appear in `corvid claim --explain` output
    /// that downstream tooling parses.
    pub const fn slug(self) -> &'static str {
        match self {
            GuaranteeKind::Approval => "approval",
            GuaranteeKind::EffectRow => "effect_row",
            GuaranteeKind::Grounded => "grounded",
            GuaranteeKind::Budget => "budget",
            GuaranteeKind::Confidence => "confidence",
            GuaranteeKind::Replay => "replay",
            GuaranteeKind::ProvenanceTrace => "provenance_trace",
            GuaranteeKind::AbiDescriptor => "abi_descriptor",
            GuaranteeKind::AbiAttestation => "abi_attestation",
            GuaranteeKind::Server => "server",
            GuaranteeKind::Jobs => "jobs",
            GuaranteeKind::Auth => "auth",
            GuaranteeKind::Connector => "connector",
            GuaranteeKind::Observability => "observability",
            GuaranteeKind::Platform => "platform",
        }
    }

    pub const ALL: &'static [GuaranteeKind] = &[
        GuaranteeKind::Approval,
        GuaranteeKind::EffectRow,
        GuaranteeKind::Grounded,
        GuaranteeKind::Budget,
        GuaranteeKind::Confidence,
        GuaranteeKind::Replay,
        GuaranteeKind::ProvenanceTrace,
        GuaranteeKind::AbiDescriptor,
        GuaranteeKind::AbiAttestation,
        GuaranteeKind::Server,
        GuaranteeKind::Jobs,
        GuaranteeKind::Auth,
        GuaranteeKind::Connector,
        GuaranteeKind::Observability,
        GuaranteeKind::Platform,
    ];
}

impl fmt::Display for GuaranteeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// Strength of the enforcement promise.
///
/// `Static` means the compiler refuses to produce a binary when the
/// invariant would be violated; the user sees a diagnostic, not a
/// runtime crash. `RuntimeChecked` means the runtime detects the
/// violation and either terminates or reports through the documented
/// channel (a non-zero exit, a specific error variant, or a refused
/// operation). `OutOfScope` is a documented promise that does NOT
/// have a check today — every `OutOfScope` entry MUST carry a
/// non-empty `out_of_scope_reason` so that the registry remains an
/// honest list of what we do and do not defend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GuaranteeClass {
    Static,
    RuntimeChecked,
    OutOfScope,
}

impl GuaranteeClass {
    pub const fn slug(self) -> &'static str {
        match self {
            GuaranteeClass::Static => "static",
            GuaranteeClass::RuntimeChecked => "runtime_checked",
            GuaranteeClass::OutOfScope => "out_of_scope",
        }
    }

    pub const ALL: &'static [GuaranteeClass] = &[
        GuaranteeClass::Static,
        GuaranteeClass::RuntimeChecked,
        GuaranteeClass::OutOfScope,
    ];
}

impl fmt::Display for GuaranteeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// Pipeline stage that owns the enforcement.
///
/// The registry uses these as a coarse taxonomy so an outsider can
/// answer "where in the build is this checked?" The slugs match
/// Corvid's actual crate layout where possible (`resolve`,
/// `typecheck`, `ir_lower`, `codegen`, `runtime`, `abi_emit`,
/// `abi_verify`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    Lex,
    Parse,
    Resolve,
    TypeCheck,
    IrLower,
    Codegen,
    Runtime,
    AbiEmit,
    AbiVerify,
    Platform,
}

impl Phase {
    pub const fn slug(self) -> &'static str {
        match self {
            Phase::Lex => "lex",
            Phase::Parse => "parse",
            Phase::Resolve => "resolve",
            Phase::TypeCheck => "typecheck",
            Phase::IrLower => "ir_lower",
            Phase::Codegen => "codegen",
            Phase::Runtime => "runtime",
            Phase::AbiEmit => "abi_emit",
            Phase::AbiVerify => "abi_verify",
            Phase::Platform => "platform",
        }
    }

    pub const ALL: &'static [Phase] = &[
        Phase::Lex,
        Phase::Parse,
        Phase::Resolve,
        Phase::TypeCheck,
        Phase::IrLower,
        Phase::Codegen,
        Phase::Runtime,
        Phase::AbiEmit,
        Phase::AbiVerify,
        Phase::Platform,
    ];
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// One row of the canonical guarantee table.
///
/// Every field is `&'static` so the entire registry is built at
/// compile time and shared across every dependent crate without
/// allocation. The `id` is the stable handle referenced by
/// diagnostics, tests, generated docs, and the CLI; renaming an `id`
/// is a breaking change to the public claim surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guarantee {
    /// Stable identifier of the form `kind.specific_promise`.
    /// Slug-cased, dot-separated, lowercase. Referenced by
    /// diagnostics and `corvid claim --explain` output.
    pub id: &'static str,
    pub kind: GuaranteeKind,
    pub class: GuaranteeClass,
    pub phase: Phase,
    /// One-line human description suitable for inclusion in
    /// generated docs and CLI output. Must remain accurate as the
    /// implementation evolves; the doc generator (Slice 35-D)
    /// emits this verbatim.
    pub description: &'static str,
    /// Why a guarantee is `OutOfScope`. Empty for `Static` and
    /// `RuntimeChecked`; non-empty (and validated) for `OutOfScope`.
    pub out_of_scope_reason: &'static str,
    /// Test functions that demonstrate the guarantee holds for
    /// valid programs. Slice 35-E enforces non-empty for `Static`
    /// and `RuntimeChecked` entries.
    pub positive_test_refs: &'static [&'static str],
    /// Test functions that demonstrate the guarantee rejects
    /// violations. Slice 35-E enforces non-empty for `Static` and
    /// `RuntimeChecked` entries.
    pub adversarial_test_refs: &'static [&'static str],
}

