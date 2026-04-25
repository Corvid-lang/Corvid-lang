//! Coroutine-style step-through execution for the Corvid REPL.
//!
//! The interpreter calls `StepHook::on_step` at each interesting point
//! (tool call, prompt call, approval gate, statement boundary). The
//! hook decides whether to continue, resume at full speed, override a
//! result, or abort. This turns the interpreter into a steerable
//! coroutine that the REPL drives interactively.

use crate::value::{value_confidence, Value};
use corvid_ast::Span;
use std::collections::HashMap;

/// Snapshot of the interpreter's local environment at a step point.
#[derive(Debug, Clone)]
pub struct EnvSnapshot {
    pub locals: Vec<(String, Value)>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct ConfidenceGateStep {
    pub threshold: f64,
    pub actual: f64,
    pub triggered: bool,
}

/// What kind of IR statement is about to execute.
#[derive(Debug, Clone)]
pub enum StmtKind {
    Let { name: String },
    Assign { name: String },
    Return,
    If,
    For { var: String },
    Approve { label: String },
    Expr,
    Break,
    Continue,
    Pass,
}

/// An event emitted by the interpreter at an interesting execution point.
#[derive(Debug, Clone)]
pub enum StepEvent {
    /// About to execute a statement (only emitted in single-step mode).
    BeforeStatement {
        kind: StmtKind,
        span: Span,
        env: EnvSnapshot,
    },

    /// About to dispatch a tool call.
    BeforeToolCall {
        tool_name: String,
        args: Vec<serde_json::Value>,
        input_confidence: f64,
        confidence_gate: Option<ConfidenceGateStep>,
        span: Span,
        env: EnvSnapshot,
    },

    /// Tool call completed.
    AfterToolCall {
        tool_name: String,
        result: serde_json::Value,
        result_confidence: f64,
        elapsed_ms: u64,
        span: Span,
    },

    /// About to dispatch a prompt (LLM) call.
    BeforePromptCall {
        prompt_name: String,
        rendered: String,
        model: Option<String>,
        input_confidence: f64,
        span: Span,
        env: EnvSnapshot,
    },

    /// Prompt call completed.
    AfterPromptCall {
        prompt_name: String,
        result: serde_json::Value,
        result_confidence: f64,
        elapsed_ms: u64,
        span: Span,
    },

    /// About to request human approval.
    BeforeApproval {
        label: String,
        args: Vec<serde_json::Value>,
        confidence_gate: Option<ConfidenceGateStep>,
        span: Span,
        env: EnvSnapshot,
    },

    /// Approval decision received.
    AfterApproval {
        label: String,
        approved: bool,
        span: Span,
    },

    /// About to call another agent.
    BeforeAgentCall {
        agent_name: String,
        args: Vec<serde_json::Value>,
        input_confidence: f64,
        span: Span,
    },

    /// Agent call completed.
    AfterAgentCall {
        agent_name: String,
        result: serde_json::Value,
        result_confidence: f64,
        span: Span,
    },

    /// Execution finished (success or error).
    Completed {
        agent_name: String,
        ok: bool,
        result: Option<Value>,
        result_confidence: Option<f64>,
        error: Option<String>,
    },
}

/// What the REPL (or any controller) tells the interpreter to do next.
#[derive(Debug, Clone)]
pub enum StepAction {
    /// Execute the next step, then pause again.
    Continue,
    /// Run to the next tool/prompt/approval event (skip statement-level pauses).
    StepOver,
    /// Run to completion without pausing.
    Resume,
    /// Replace the result of the last tool/prompt call with this value.
    Override(serde_json::Value),
    /// At a BeforeApproval event: approve the request (skip the runtime gate).
    Approve,
    /// At a BeforeApproval event: deny the request (skip the runtime gate).
    Deny,
    /// Abort execution immediately.
    Abort,
}

/// Trait for controlling interpreter execution from the REPL.
///
/// Implementations decide the pacing: a REPL step-through controller
/// waits for user input; a test controller can script a sequence of
/// actions; normal execution uses `NoOpHook` which always resumes.
#[async_trait::async_trait]
pub trait StepHook: Send + Sync {
    async fn on_step(&self, event: &StepEvent) -> StepAction;
}

/// Default hook: never pauses, always resumes at full speed.
pub struct NoOpHook;

#[async_trait::async_trait]
impl StepHook for NoOpHook {
    async fn on_step(&self, _event: &StepEvent) -> StepAction {
        StepAction::Resume
    }
}

/// Stepping mode — controls which events the interpreter yields on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMode {
    /// Pause at every statement.
    Statement,
    /// Pause only at tool/prompt/approval boundaries.
    Boundary,
    /// Don't pause (normal execution).
    Run,
}

/// Mutable stepping state held by the interpreter.
pub struct StepController {
    pub mode: StepMode,
    hook: std::sync::Arc<dyn StepHook>,
}

impl StepController {
    pub fn new(hook: std::sync::Arc<dyn StepHook>, mode: StepMode) -> Self {
        Self { mode, hook }
    }

    pub fn hook_ref(&self) -> std::sync::Arc<dyn StepHook> {
        std::sync::Arc::clone(&self.hook)
    }

    pub fn should_yield_on_statement(&self) -> bool {
        self.mode == StepMode::Statement
    }

    pub fn should_yield_on_boundary(&self) -> bool {
        self.mode != StepMode::Run
    }

    pub async fn yield_event(&mut self, event: StepEvent) -> StepAction {
        let action = self.hook.on_step(&event).await;
        match action {
            StepAction::Resume => {
                self.mode = StepMode::Run;
            }
            StepAction::StepOver => {
                self.mode = StepMode::Boundary;
            }
            StepAction::Continue => {
                self.mode = StepMode::Statement;
            }
            StepAction::Override(_) | StepAction::Approve | StepAction::Deny | StepAction::Abort => {}
        }
        action
    }
}

/// Build an `EnvSnapshot` from the interpreter's current local bindings.
/// Requires a name-resolution map to turn `LocalId` into human-readable names.
pub fn snapshot_env(
    env: &crate::env::Env,
    names: &HashMap<corvid_resolve::LocalId, String>,
) -> EnvSnapshot {
    let mut locals: Vec<(String, Value)> = names
        .iter()
        .filter_map(|(id, name)| env.lookup(*id).map(|v| (name.clone(), v)))
        .collect();
    locals.sort_by(|a, b| a.0.cmp(&b.0));
    let confidence = locals
        .iter()
        .map(|(_, value)| value_confidence(value))
        .fold(1.0_f64, f64::min);
    EnvSnapshot { locals, confidence }
}

// ---- Execution trace: recording + replay + fork ----

/// One recorded checkpoint during execution.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub index: usize,
    pub event: StepEvent,
    pub action: StepAction,
}

/// Complete execution trace — every step event and the user/controller
/// response. This is the journal for replay-fork-explore.
#[derive(Debug, Clone, Default)]
pub struct ExecutionTrace {
    pub checkpoints: Vec<Checkpoint>,
}

impl ExecutionTrace {
    pub fn new() -> Self {
        Self { checkpoints: Vec::new() }
    }

    pub fn record(&mut self, event: StepEvent, action: StepAction) {
        let index = self.checkpoints.len();
        self.checkpoints.push(Checkpoint { index, event, action });
    }

    pub fn len(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    /// Find the first checkpoint matching a tool name.
    pub fn find_tool_call(&self, tool_name: &str) -> Option<usize> {
        self.checkpoints.iter().position(|cp| matches!(
            &cp.event,
            StepEvent::BeforeToolCall { tool_name: t, .. } if t == tool_name
        ))
    }

    /// Find the first checkpoint matching a prompt name.
    pub fn find_prompt_call(&self, prompt_name: &str) -> Option<usize> {
        self.checkpoints.iter().position(|cp| matches!(
            &cp.event,
            StepEvent::BeforePromptCall { prompt_name: p, .. } if p == prompt_name
        ))
    }

    /// Find the first approval checkpoint matching a label.
    pub fn find_approval(&self, label: &str) -> Option<usize> {
        self.checkpoints.iter().position(|cp| matches!(
            &cp.event,
            StepEvent::BeforeApproval { label: l, .. } if l == label
        ))
    }

    /// Boundary checkpoints only (tool/prompt/approval/agent calls).
    pub fn boundaries(&self) -> Vec<&Checkpoint> {
        self.checkpoints.iter().filter(|cp| matches!(
            cp.event,
            StepEvent::BeforeToolCall { .. }
            | StepEvent::AfterToolCall { .. }
            | StepEvent::BeforePromptCall { .. }
            | StepEvent::AfterPromptCall { .. }
            | StepEvent::BeforeApproval { .. }
            | StepEvent::AfterApproval { .. }
            | StepEvent::BeforeAgentCall { .. }
            | StepEvent::AfterAgentCall { .. }
        )).collect()
    }
}

/// A hook that records every step event into a trace while delegating
/// the actual decision to an inner hook.
pub struct RecordingHook {
    inner: std::sync::Arc<dyn StepHook>,
    trace: std::sync::Arc<std::sync::Mutex<ExecutionTrace>>,
}

impl RecordingHook {
    pub fn new(inner: std::sync::Arc<dyn StepHook>) -> Self {
        Self {
            inner,
            trace: std::sync::Arc::new(std::sync::Mutex::new(ExecutionTrace::new())),
        }
    }

    pub fn trace(&self) -> ExecutionTrace {
        self.trace.lock().unwrap().clone()
    }

    pub fn trace_ref(&self) -> std::sync::Arc<std::sync::Mutex<ExecutionTrace>> {
        std::sync::Arc::clone(&self.trace)
    }
}

#[async_trait::async_trait]
impl StepHook for RecordingHook {
    async fn on_step(&self, event: &StepEvent) -> StepAction {
        let action = self.inner.on_step(event).await;
        self.trace.lock().unwrap().record(event.clone(), action.clone());
        action
    }
}

/// A hook that replays a recorded trace up to a fork point, then
/// switches to a live hook for the remainder. This is the engine
/// behind `:whatif` and `:fork`.
///
/// During replay (step < fork_at): feeds back recorded Override/Resume
/// actions so the interpreter re-executes with the same tool/prompt
/// results without making live calls.
///
/// At the fork point: applies the override (if any), then switches to
/// the live hook for all subsequent steps.
///
/// After fork (step >= fork_at): delegates to the live hook.
pub struct ReplayForkHook {
    trace: ExecutionTrace,
    fork_at: usize,
    fork_override: Option<serde_json::Value>,
    live_hook: std::sync::Arc<dyn StepHook>,
    current_step: std::sync::atomic::AtomicUsize,
}

impl ReplayForkHook {
    pub fn new(
        trace: ExecutionTrace,
        fork_at: usize,
        fork_override: Option<serde_json::Value>,
        live_hook: std::sync::Arc<dyn StepHook>,
    ) -> Self {
        Self {
            trace,
            fork_at,
            fork_override,
            live_hook,
            current_step: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl StepHook for ReplayForkHook {
    async fn on_step(&self, event: &StepEvent) -> StepAction {
        let step = self.current_step.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        if step == self.fork_at {
            if let Some(ref override_val) = self.fork_override {
                return StepAction::Override(override_val.clone());
            }
            return self.live_hook.on_step(event).await;
        }

        if step < self.fork_at {
            // During replay: feed back the recorded action. For AfterToolCall
            // / AfterPromptCall events, the result is already baked into the
            // interpreter's re-execution path (the Override on the Before
            // event handled it). Just resume through these.
            if let Some(cp) = self.trace.checkpoints.get(step) {
                match &cp.event {
                    StepEvent::BeforeToolCall { .. }
                    | StepEvent::BeforePromptCall { .. } => {
                        // During replay, feed the recorded AfterToolCall/
                        // AfterPromptCall result as an override so the
                        // interpreter doesn't make a live call.
                        if let Some(next_cp) = self.trace.checkpoints.get(step + 1) {
                            match &next_cp.event {
                                StepEvent::AfterToolCall { result, .. }
                                | StepEvent::AfterPromptCall { result, .. } => {
                                    return StepAction::Override(result.clone());
                                }
                                _ => {}
                            }
                        }
                        return StepAction::Resume;
                    }
                    StepEvent::BeforeApproval { .. } => {
                        // Replay the recorded approval decision.
                        if let Some(next_cp) = self.trace.checkpoints.get(step + 1) {
                            if let StepEvent::AfterApproval { approved, .. } = &next_cp.event {
                                return if *approved { StepAction::Approve } else { StepAction::Deny };
                            }
                        }
                        return StepAction::Approve;
                    }
                    _ => return StepAction::Resume,
                }
            }
            return StepAction::Resume;
        }

        // After fork: live execution.
        self.live_hook.on_step(event).await
    }
}
