//! Effect classification for tools and agents.
//!
//! v0.1 uses a binary system: `Safe` / `Dangerous`.
//!
//! v0.6 (Phase 20) extends this with dimensional effects: each effect
//! declaration carries typed dimensions (cost, trust, reversibility,
//! data classification, latency) that compose independently through
//! the call graph.
//!
//! The `Effect` enum is kept for backward compatibility. New code
//! uses `EffectRef` (a reference to a declared dimensional effect)
//! alongside the legacy enum.

use crate::span::{Ident, Span};
use serde::{Deserialize, Serialize};

/// Legacy binary effect classification. Retained for backward
/// compatibility — `dangerous` keyword on tool declarations still
/// produces `Effect::Dangerous`. The typechecker maps this to the
/// dimensional system internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Effect {
    Safe,
    Dangerous,
}

// ---- Dimensional effect system (Phase 20) ----

/// A dimension value in an effect declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DimensionValue {
    /// A boolean dimension: `reversible: true`.
    Bool(bool),
    /// A string/enum dimension: `trust: human_required`.
    Name(String),
    /// A cost dimension: `cost: $0.001`.
    Cost(f64),
    /// A numeric dimension: `latency_ms: 100`.
    Number(f64),
}

/// A single dimension declaration inside an `effect` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionDecl {
    pub name: Ident,
    pub value: DimensionValue,
    pub span: Span,
}

/// A top-level `effect` declaration with dimensions.
///
/// ```text
/// effect transfer_money:
///     cost: $0.001
///     reversible: false
///     trust: human_required
///     data: financial
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectDecl {
    pub name: Ident,
    pub dimensions: Vec<DimensionDecl>,
    pub span: Span,
}

/// A reference to a declared effect, used in `uses` clauses.
///
/// ```text
/// tool issue_refund(...) -> Receipt uses transfer_money
/// agent planner(...) -> Plan uses search_knowledge, transfer_money
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectRef {
    pub name: Ident,
    pub span: Span,
}

/// An effect row on a declaration — the list of effects it uses.
///
/// ```text
/// agent planner(...) -> Plan uses search_knowledge, transfer_money
/// ```
///
/// If empty, the declaration has no declared effects (equivalent to
/// the legacy `Safe`). The typechecker infers the actual effect row
/// from the body for agents; tools must declare explicitly.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EffectRow {
    pub effects: Vec<EffectRef>,
    pub span: Span,
}

impl EffectRow {
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn names(&self) -> Vec<&str> {
        self.effects.iter().map(|e| e.name.name.as_str()).collect()
    }
}

/// A constraint annotation on an agent or block.
///
/// ```text
/// @budget($1.00)
/// @trust(autonomous)
/// @reversible
/// @latency(fast)
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectConstraint {
    pub dimension: Ident,
    pub value: Option<DimensionValue>,
    pub span: Span,
}

/// Composition rule for a dimension when effects combine through
/// a call chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositionRule {
    /// Sum values along the call path (cost).
    Sum,
    /// Take the maximum / most restrictive (trust, latency).
    Max,
    /// Take the minimum / least restrictive (confidence).
    Min,
    /// Union the values (data classification).
    Union,
    /// Take the least reversible (reversibility).
    LeastReversible,
}

/// A dimension schema — defines a named dimension with its type,
/// default value, and composition rule. Registered globally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionSchema {
    pub name: String,
    pub composition: CompositionRule,
    pub default: DimensionValue,
}
