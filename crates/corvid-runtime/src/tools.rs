//! Tool registry and dispatch.
//!
//! Tools are registered by name with an async handler. The handler takes a
//! JSON array of arguments and returns a JSON value. Speaking JSON at the
//! boundary keeps the runtime independent of the interpreter's `Value`
//! type and matches the wire format of every real LLM tool protocol.

use crate::errors::RuntimeError;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;

/// Type-erased async tool handler. Cheap to clone (it's an `Arc`).
pub type ToolHandler = Arc<
    dyn Fn(Vec<serde_json::Value>) -> BoxFuture<'static, Result<serde_json::Value, RuntimeError>>
        + Send
        + Sync,
>;

/// Map of tool name → handler.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, ToolHandler>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. The handler is any closure returning a future.
    ///
    /// ```ignore
    /// registry.register("get_order", |args| async move {
    ///     let id = args[0].as_str().unwrap_or("");
    ///     Ok(serde_json::json!({ "id": id, "amount": 19.99 }))
    /// });
    /// ```
    pub fn register<F, Fut>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<serde_json::Value, RuntimeError>>
            + Send
            + 'static,
    {
        let boxed: ToolHandler = Arc::new(move |args| Box::pin(handler(args)));
        self.tools.insert(name.into(), boxed);
    }

    /// Dispatch a tool call. Returns `UnknownTool` if no handler exists.
    pub async fn call(
        &self,
        name: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, RuntimeError> {
        let handler = self
            .tools
            .get(name)
            .ok_or_else(|| RuntimeError::UnknownTool(name.to_string()))?
            .clone();
        handler(args).await
    }

    /// Whether a handler is registered for `name`. Useful for diagnostics.
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Names of all registered tools, alphabetized. Used by tracing.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tools.keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn register_and_call() {
        let mut reg = ToolRegistry::new();
        reg.register("double", |args| async move {
            let n = args[0].as_i64().unwrap();
            Ok(json!(n * 2))
        });
        let out = reg.call("double", vec![json!(21)]).await.unwrap();
        assert_eq!(out, json!(42));
    }

    #[tokio::test]
    async fn unknown_tool_errors() {
        let reg = ToolRegistry::new();
        let err = reg.call("missing", vec![]).await.unwrap_err();
        assert!(matches!(err, RuntimeError::UnknownTool(ref s) if s == "missing"));
    }

    #[tokio::test]
    async fn handler_error_propagates() {
        let mut reg = ToolRegistry::new();
        reg.register("nope", |_| async {
            Err(RuntimeError::ToolFailed {
                tool: "nope".into(),
                message: "boom".into(),
            })
        });
        let err = reg.call("nope", vec![]).await.unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::ToolFailed { ref tool, .. } if tool == "nope"
        ));
    }
}
