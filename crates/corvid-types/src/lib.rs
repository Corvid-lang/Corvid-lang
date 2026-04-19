//! Type system and effect checker.
//!
//! Walks a parsed, resolved `File` and validates type and effect rules.
//! The headline check is **approve-before-dangerous**: any call to a tool
//! declared `dangerous` must be preceded by a matching `approve` in the
//! same block, or compilation fails.
//!
//! See `ARCHITECTURE.md` §5–§6.

#![allow(dead_code)]

pub mod checker;
pub mod config;
pub mod effects;
pub mod errors;
pub mod law_check;
pub mod repl;
pub mod types;

pub use checker::{typecheck, typecheck_with_config, Checked};
pub use config::{
    CorvidConfig, CustomDimensionConfig, CustomDimensionMeta, DimensionConfigError,
    DimensionValueType, EffectSystemConfig, BUILTIN_DIMENSION_NAMES,
};
pub use law_check::{
    check_dimension, laws_for_rule, DimensionUnderTest, Law, LawCheckResult, Verdict,
    DEFAULT_SAMPLES,
};
pub use errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
pub use repl::{CheckedTurn, ReplLocal, ReplSession, ReplTurnBuild, REPL_RESULT_NAME};
pub use effects::{
    analyze_effects, check_grounded_returns, compose_dimension_public,
    compute_worst_case_cost, render_cost_tree, AgentEffectSummary, ComposedProfile,
    ConstraintViolation, CostEstimate, CostNodeKind, CostTreeNode, CostWarning,
    CostWarningKind, EffectProfile, EffectRegistry, ProvenanceViolation,
};
pub use types::Type;


#[cfg(test)]
mod tests;
