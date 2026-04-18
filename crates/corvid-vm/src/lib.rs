//! Tree-walking interpreter for the Corvid IR.
//!
//! Two roles:
//!
//! 1. **Dev tier.** During development, `corvid run` dispatches through this
//!    interpreter so changes show up without a native recompile step.
//! 2. **Correctness oracle.** Once the Cranelift native compiler is in
//!    flight, compiler output is validated against interpreter output
//!    for every fixture — which is why this tier is async-native, matching
//!    the future native runtime instead of taking the easier sync route.
//!
//! Side-effecting work (tool dispatch, LLM calls, approvals, tracing) is
//! delegated to `corvid-runtime`. The interpreter converts between
//! `Value` and `serde_json::Value` at the boundary (`crate::conv`).
//!
//! See `ARCHITECTURE.md` §4 (pipeline).

#![forbid(unsafe_code)]
#![allow(dead_code)]

pub mod conv;
pub mod cycle_collector;
pub mod env;
pub mod errors;
pub mod interp;
pub mod repl_display;
pub mod schema;
pub mod step;
pub mod value;

pub use conv::{json_to_value, value_to_json, ConvError};
pub use cycle_collector::collect_cycles;
pub use env::Env;
pub use errors::{InterpError, InterpErrorKind};
pub use interp::{bind_and_run_agent, build_struct, run_agent, run_agent_stepping, run_agent_with_env};
pub use repl_display::render_value;
pub use schema::schema_for;
pub use step::{
    Checkpoint, EnvSnapshot, ExecutionTrace, NoOpHook, RecordingHook, ReplayForkHook,
    StepAction, StepController, StepEvent, StepHook, StepMode, StmtKind,
};
pub use value::{
    GroundedValue, ProvenanceChain, ProvenanceEntry, ProvenanceKind, StreamValue, StructValue,
    Value,
};

#[cfg(test)]
mod tests;
