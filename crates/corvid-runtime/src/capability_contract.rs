use crate::errors::RuntimeError;
use crate::llm::LlmRequest;
use crate::runtime::Runtime;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityCheckKind {
    StructuredOutput,
    TokenUsage,
    ContextWindow,
    ToolCalling,
    Streaming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityCheckStatus {
    Passed,
    Failed,
    Skipped,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityContractCheck {
    pub model: String,
    pub provider: Option<String>,
    pub kind: CapabilityCheckKind,
    pub status: CapabilityCheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityContractOptions {
    pub live_structured_output_probe: bool,
    pub require_token_usage: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityContractReport {
    pub checks: Vec<CapabilityContractCheck>,
}

impl CapabilityContractReport {
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != CapabilityCheckStatus::Failed)
    }
}

pub async fn run_capability_contracts(
    runtime: &Runtime,
    options: CapabilityContractOptions,
) -> Result<CapabilityContractReport, RuntimeError> {
    let mut checks = Vec::new();
    for name in runtime.model_catalog().names() {
        let Some(model) = runtime.model_catalog().get(&name).cloned() else {
            continue;
        };
        let provider = model.provider.clone();
        if model.structured_output {
            if options.live_structured_output_probe {
                let response = runtime
                    .call_llm(LlmRequest {
                        prompt: "__corvid_capability_contract_structured_output".to_string(),
                        model: model.name.clone(),
                        rendered: "Return JSON matching the provided schema with ok=true."
                            .to_string(),
                        args: vec![],
                        output_schema: Some(json!({
                            "type": "object",
                            "properties": {
                                "ok": { "type": "boolean" }
                            },
                            "required": ["ok"],
                            "additionalProperties": false
                        })),
                    })
                    .await;
                match response {
                    Ok(response) => {
                        let passed = response
                            .value
                            .as_object()
                            .and_then(|object| object.get("ok"))
                            .and_then(serde_json::Value::as_bool)
                            == Some(true);
                        checks.push(CapabilityContractCheck {
                            model: model.name.clone(),
                            provider: provider.clone(),
                            kind: CapabilityCheckKind::StructuredOutput,
                            status: if passed {
                                CapabilityCheckStatus::Passed
                            } else {
                                CapabilityCheckStatus::Failed
                            },
                            message: if passed {
                                "adapter returned schema-shaped JSON".to_string()
                            } else {
                                format!(
                                    "adapter returned value outside the structured-output contract: {}",
                                    response.value
                                )
                            },
                        });
                        if options.require_token_usage {
                            push_token_usage_check(
                                &mut checks,
                                &model.name,
                                provider.clone(),
                                response.usage.prompt_tokens,
                                response.usage.completion_tokens,
                                response.usage.total_tokens,
                            );
                        }
                    }
                    Err(err) => checks.push(CapabilityContractCheck {
                        model: model.name.clone(),
                        provider: provider.clone(),
                        kind: CapabilityCheckKind::StructuredOutput,
                        status: CapabilityCheckStatus::Failed,
                        message: err.to_string(),
                    }),
                }
            } else {
                checks.push(CapabilityContractCheck {
                    model: model.name.clone(),
                    provider: provider.clone(),
                    kind: CapabilityCheckKind::StructuredOutput,
                    status: CapabilityCheckStatus::Skipped,
                    message: "live structured-output probe disabled".to_string(),
                });
            }
        }
        if let Some(context_window) = model.context_window {
            checks.push(CapabilityContractCheck {
                model: model.name.clone(),
                provider: provider.clone(),
                kind: CapabilityCheckKind::ContextWindow,
                status: if context_window > 0 {
                    CapabilityCheckStatus::Passed
                } else {
                    CapabilityCheckStatus::Failed
                },
                message: format!("declared context window: {context_window} tokens"),
            });
        }
        if model.tool_calling {
            checks.push(CapabilityContractCheck {
                model: model.name.clone(),
                provider: provider.clone(),
                kind: CapabilityCheckKind::ToolCalling,
                status: CapabilityCheckStatus::Unsupported,
                message: "LLM adapter surface does not expose provider-native tool-call probes yet"
                    .to_string(),
            });
        }
        checks.push(CapabilityContractCheck {
            model: model.name.clone(),
            provider,
            kind: CapabilityCheckKind::Streaming,
            status: CapabilityCheckStatus::Unsupported,
            message: "LLM adapter surface does not expose provider-native streaming probes yet"
                .to_string(),
        });
    }
    Ok(CapabilityContractReport { checks })
}

fn push_token_usage_check(
    checks: &mut Vec<CapabilityContractCheck>,
    model: &str,
    provider: Option<String>,
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
) {
    let passed = prompt_tokens > 0
        && completion_tokens > 0
        && total_tokens >= prompt_tokens.saturating_add(completion_tokens);
    checks.push(CapabilityContractCheck {
        model: model.to_string(),
        provider,
        kind: CapabilityCheckKind::TokenUsage,
        status: if passed {
            CapabilityCheckStatus::Passed
        } else {
            CapabilityCheckStatus::Failed
        },
        message: format!(
            "reported prompt={prompt_tokens}, completion={completion_tokens}, total={total_tokens}"
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::ProgrammaticApprover;
    use crate::llm::{mock::MockAdapter, TokenUsage};
    use crate::models::RegisteredModel;
    use std::sync::Arc;

    #[tokio::test]
    async fn live_structured_output_probe_reports_pass_and_token_usage() {
        let runtime = Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(MockAdapter::new("json-model").reply_with_usage(
                "__corvid_capability_contract_structured_output",
                json!({"ok": true}),
                TokenUsage {
                    prompt_tokens: 8,
                    completion_tokens: 2,
                    total_tokens: 10,
                },
            )))
            .model(
                RegisteredModel::new("json-model")
                    .provider("mock")
                    .structured_output(true)
                    .context_window(4096),
            )
            .build();

        let report = run_capability_contracts(
            &runtime,
            CapabilityContractOptions {
                live_structured_output_probe: true,
                require_token_usage: true,
            },
        )
        .await
        .unwrap();

        assert!(report.passed());
        assert!(report.checks.iter().any(|check| {
            check.kind == CapabilityCheckKind::StructuredOutput
                && check.status == CapabilityCheckStatus::Passed
        }));
        assert!(report.checks.iter().any(|check| {
            check.kind == CapabilityCheckKind::TokenUsage
                && check.status == CapabilityCheckStatus::Passed
        }));
        assert!(report.checks.iter().any(|check| {
            check.kind == CapabilityCheckKind::ContextWindow
                && check.status == CapabilityCheckStatus::Passed
        }));
    }

    #[tokio::test]
    async fn live_structured_output_probe_reports_contract_failure() {
        let runtime = Runtime::builder()
            .llm(Arc::new(
                MockAdapter::new("json-model").reply(
                    "__corvid_capability_contract_structured_output",
                    json!("not-json-object"),
                ),
            ))
            .model(
                RegisteredModel::new("json-model")
                    .provider("mock")
                    .structured_output(true),
            )
            .build();

        let report = run_capability_contracts(
            &runtime,
            CapabilityContractOptions {
                live_structured_output_probe: true,
                require_token_usage: false,
            },
        )
        .await
        .unwrap();

        assert!(!report.passed());
        assert!(report.checks.iter().any(|check| {
            check.kind == CapabilityCheckKind::StructuredOutput
                && check.status == CapabilityCheckStatus::Failed
        }));
    }
}
