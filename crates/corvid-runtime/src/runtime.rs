//! `Runtime` — the top-level glue the interpreter calls into.
//!
//! Bundles the tool registry, LLM registry, approver, and tracer behind
//! one handle. Construct with `Runtime::builder()`, populate with tools
//! and adapters, freeze with `.build()`. Pass `&Runtime` to the
//! interpreter.

use crate::approvals::{Approver, ApprovalDecision, ApprovalRequest, StdinApprover};
use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRegistry, LlmRequest, LlmResponse};
use crate::tools::ToolRegistry;
use crate::tracing::{fresh_run_id, now_ms, TraceEvent, Tracer};
use std::path::Path;
use std::sync::Arc;

pub struct Runtime {
    tools: ToolRegistry,
    llms: LlmRegistry,
    approver: Arc<dyn Approver>,
    tracer: Tracer,
    /// Default model name applied when a prompt call doesn't specify one.
    /// Empty string means "no default; require per-call override".
    default_model: String,
}

impl Runtime {
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    // ---- accessors used by the interpreter ----

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn tracer(&self) -> &Tracer {
        &self.tracer
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    // ---- dispatch helpers ----

    /// Call a tool by name. Emits trace events bracketing the call.
    pub async fn call_tool(
        &self,
        name: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, RuntimeError> {
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::ToolCall {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                tool: name.to_string(),
                args: args.clone(),
            });
        }
        let result = self.tools.call(name, args).await?;
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::ToolResult {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                tool: name.to_string(),
                result: result.clone(),
            });
        }
        Ok(result)
    }

    /// Call an LLM. Falls back to `default_model` if `req.model` is empty.
    pub async fn call_llm(&self, mut req: LlmRequest) -> Result<LlmResponse, RuntimeError> {
        if req.model.is_empty() {
            req.model = self.default_model.clone();
        }
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::LlmCall {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                prompt: req.prompt.clone(),
                model: if req.model.is_empty() {
                    None
                } else {
                    Some(req.model.clone())
                },
                rendered: Some(req.rendered.clone()),
                args: req.args.clone(),
            });
        }
        let resp = self.llms.call(&req).await?;
        if self.tracer.is_enabled() {
            self.tracer.emit(TraceEvent::LlmResult {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                prompt: req.prompt.clone(),
                result: resp.value.clone(),
            });
        }
        Ok(resp)
    }

    /// Ask the approver about an action. Returns `ApprovalDenied` if
    /// denied; the interpreter surfaces this as `InterpError::Runtime`.
    pub async fn approval_gate(
        &self,
        label: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<(), RuntimeError> {
        let trace_enabled = self.tracer.is_enabled();
        let label_owned = label.to_string();
        if trace_enabled {
            self.tracer.emit(TraceEvent::ApprovalRequest {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                label: label_owned.clone(),
                args: args.clone(),
            });
        }
        let req = ApprovalRequest {
            label: label_owned.clone(),
            args,
        };
        let decision = self.approver.approve(&req).await?;
        let approved = decision == ApprovalDecision::Approve;
        if trace_enabled {
            self.tracer.emit(TraceEvent::ApprovalResponse {
                ts_ms: now_ms(),
                run_id: self.tracer.run_id().to_string(),
                label: label_owned.clone(),
                approved,
            });
        }
        if approved {
            Ok(())
        } else {
            Err(RuntimeError::ApprovalDenied {
                action: label_owned,
            })
        }
    }
}

#[derive(Default)]
pub struct RuntimeBuilder {
    tools: ToolRegistry,
    llms: LlmRegistry,
    approver: Option<Arc<dyn Approver>>,
    tracer: Option<Tracer>,
    default_model: String,
}

impl RuntimeBuilder {
    pub fn tool<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<serde_json::Value, RuntimeError>>
            + Send
            + 'static,
    {
        self.tools.register(name, handler);
        self
    }

    pub fn llm(mut self, adapter: Arc<dyn LlmAdapter>) -> Self {
        self.llms.register(adapter);
        self
    }

    pub fn approver(mut self, approver: Arc<dyn Approver>) -> Self {
        self.approver = Some(approver);
        self
    }

    pub fn tracer(mut self, tracer: Tracer) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Open a JSONL trace file under `dir` with a fresh run id.
    pub fn trace_to(self, dir: &Path) -> Self {
        let tracer = Tracer::open(dir, fresh_run_id());
        self.tracer(tracer)
    }

    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    pub fn build(self) -> Runtime {
        Runtime {
            tools: self.tools,
            llms: self.llms,
            approver: self
                .approver
                .unwrap_or_else(|| Arc::new(StdinApprover::new())),
            tracer: self.tracer.unwrap_or_else(Tracer::null),
            default_model: self.default_model,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::ProgrammaticApprover;
    use crate::llm::mock::MockAdapter;
    use serde_json::json;

    fn rt() -> Runtime {
        Runtime::builder()
            .tool("double", |args| async move {
                let n = args[0].as_i64().unwrap();
                Ok(json!(n * 2))
            })
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(
                MockAdapter::new("mock-1").reply("greet", json!("hi")),
            ))
            .default_model("mock-1")
            .build()
    }

    #[tokio::test]
    async fn call_tool_routes_through_registry() {
        let r = rt();
        let v = r.call_tool("double", vec![json!(5)]).await.unwrap();
        assert_eq!(v, json!(10));
    }

    #[tokio::test]
    async fn approval_gate_passes_when_approver_says_yes() {
        let r = rt();
        r.approval_gate("Anything", vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn approval_gate_blocks_when_approver_says_no() {
        let r = Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_no()))
            .build();
        let err = r.approval_gate("IssueRefund", vec![]).await.unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::ApprovalDenied { ref action } if action == "IssueRefund"
        ));
    }

    #[tokio::test]
    async fn call_llm_uses_default_model_when_request_blank() {
        let r = rt();
        let resp = r
            .call_llm(LlmRequest {
                prompt: "greet".into(),
                model: String::new(),
                rendered: "say hi".into(),
                args: vec![],
                output_schema: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.value, json!("hi"));
    }
}
