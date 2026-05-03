use corvid_abi::{emit_abi, AbiAgent, EmitOptions, TypeDescription};
use corvid_ast::{AgentDecl, BinaryOp, Decl, Expr, ExternAbi, File, Literal, Stmt, UnaryOp};
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck_with_config, CorvidConfig, EffectRegistry};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

mod compile;
mod simulate;
mod state;

use compile::{compile_approver_source, validate_approver_safety};
use simulate::MiniApproverProgram;
pub use simulate::{simulate_approver_source, SimulatedApproverDecision};
pub(crate) use state::registered_approver_overlay;
use state::RegisteredApprover;
pub use state::{
    clear_registered_approver, evaluate_registered_approver, register_approver_from_source,
};

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
