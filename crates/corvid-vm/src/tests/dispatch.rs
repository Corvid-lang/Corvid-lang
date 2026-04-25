use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct BoundaryHook;

#[derive(Clone)]
struct CountingAdapter {
    model: String,
    calls: Arc<AtomicUsize>,
    response: serde_json::Value,
}

impl corvid_runtime::LlmAdapter for CountingAdapter {
    fn name(&self) -> &str {
        &self.model
    }

    fn handles(&self, model: &str) -> bool {
        model == self.model
    }

    fn call<'a>(
        &'a self,
        _req: &'a corvid_runtime::llm::LlmRequestRef<'a>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<corvid_runtime::LlmResponse, RuntimeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(corvid_runtime::LlmResponse::new(
                self.response.clone(),
                TokenUsage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                },
            ))
        })
    }
}

#[async_trait::async_trait]
impl StepHook for BoundaryHook {
    async fn on_step(&self, _event: &StepEvent) -> StepAction {
        StepAction::StepOver
    }
}

// ---------------------------- Runtime integration ----------------------

#[tokio::test]
async fn tool_call_with_no_handler_surfaces_unknown_tool() {
    let src = "\
tool echo(x: String) -> String

agent caller(s: String) -> String:
    return echo(s)
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let err = run_agent(&ir, "caller", vec![Value::String(Arc::from("hi"))], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::UnknownTool(ref name)) => {
            assert_eq!(name, "echo");
        }
        other => panic!("expected Runtime(UnknownTool), got {other:?}"),
    }
}

#[tokio::test]
async fn tool_call_with_registered_handler_returns_value() {
    let src = "\
tool double(x: Int) -> Int

agent run(n: Int) -> Int:
    return double(n)
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("double", |args| async move {
            let n = args[0].as_i64().unwrap();
            Ok(json!(n * 2))
        })
        .build();
    let v = run_agent(&ir, "run", vec![Value::Int(21)], &rt).await.expect("run");
    assert_eq!(v, Value::Int(42));
}

#[tokio::test]
async fn grounded_unwrap_discarding_sources_returns_inner_value() {
    let src = "\
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent run(id: String) -> String:
    doc = fetch_doc(id)
    return doc.unwrap_discarding_sources()
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("fetch_doc", |args| async move {
            let id = args[0].as_str().unwrap();
            Ok(json!(format!("doc:{id}")))
        })
        .build();
    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("42"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("doc:42")));
}

#[tokio::test]
async fn approve_then_dangerous_tool_call_succeeds_with_yes_approver() {
    let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent run(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("issue_refund", |args| async move {
            let id = args[0].as_str().unwrap_or("");
            Ok(json!({"id": id}))
        })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let v = run_agent(
        &ir,
        "run",
        vec![Value::String(Arc::from("ord_1")), Value::Float(99.99)],
        &rt,
    )
    .await
    .expect("run");
    match v {
        Value::Struct(s) => {
            assert_eq!(s.type_name(), "Receipt");
            assert_eq!(s.get_field("id").unwrap(), Value::String(Arc::from("ord_1")));
        }
        other => panic!("expected Receipt struct, got {other:?}"),
    }
}

#[tokio::test]
async fn approve_with_no_approver_denial_surfaces_as_runtime_error() {
    let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent run(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("issue_refund", |_| async move {
            Ok(json!({"id": "should_never_happen"}))
        })
        .approver(Arc::new(ProgrammaticApprover::always_no()))
        .build();
    let err = run_agent(
        &ir,
        "run",
        vec![Value::String(Arc::from("ord_1")), Value::Float(99.99)],
        &rt,
    )
    .await
    .unwrap_err();
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::ApprovalDenied { ref action }) => {
            assert_eq!(action, "IssueRefund");
        }
        other => panic!("expected Runtime(ApprovalDenied), got {other:?}"),
    }
}

#[tokio::test]
async fn confidence_gate_below_threshold_asks_approver_and_continues_when_approved() {
    let src = r#"
effect gated_action:
    trust: autonomous_if_confident(0.90)

tool act(label: String) -> String uses gated_action

@trust(autonomous)
agent run(label: Grounded<String>) -> String:
    return act(label)
"#;
    let ir = ir_of(src);
    let approvals = Arc::new(AtomicUsize::new(0));
    let approvals_for_closure = Arc::clone(&approvals);
    let rt = Runtime::builder()
        .tool("act", |args| async move {
            Ok(json!(format!("acted:{}", args[0]["value"].as_str().unwrap())))
        })
        .approver(Arc::new(ProgrammaticApprover::new(move |req| {
            approvals_for_closure.fetch_add(1, Ordering::SeqCst);
            assert_eq!(req.label, "ConfidenceGate:act");
            ApprovalDecision::Approve
        })))
        .build();

    let input = Value::Grounded(GroundedValue::with_confidence(
        Value::String(Arc::from("refund")),
        ProvenanceChain::new(),
        0.70,
    ));
    let value = run_agent(&ir, "run", vec![input], &rt)
        .await
        .expect("run");

    assert_eq!(value, Value::String(Arc::from("acted:refund")));
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn confidence_gate_below_threshold_denies_when_approver_denies() {
    let src = r#"
effect gated_action:
    trust: autonomous_if_confident(0.90)

tool act(label: String) -> String uses gated_action

@trust(autonomous)
agent run(label: Grounded<String>) -> String:
    return act(label)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("act", |_| async move {
            Ok(json!("should_not_run"))
        })
        .approver(Arc::new(ProgrammaticApprover::always_no()))
        .build();

    let input = Value::Grounded(GroundedValue::with_confidence(
        Value::String(Arc::from("refund")),
        ProvenanceChain::new(),
        0.70,
    ));
    let err = run_agent(&ir, "run", vec![input], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::ApprovalDenied { ref action }) => {
            assert_eq!(action, "ConfidenceGate:act");
        }
        other => panic!("expected Runtime(ApprovalDenied), got {other:?}"),
    }
}

#[tokio::test]
async fn confidence_gate_above_threshold_skips_approver() {
    let src = r#"
effect gated_action:
    trust: autonomous_if_confident(0.90)

tool act(label: String) -> String uses gated_action

@trust(autonomous)
agent run(label: Grounded<String>) -> String:
    return act(label)
"#;
    let ir = ir_of(src);
    let approvals = Arc::new(AtomicUsize::new(0));
    let approvals_for_closure = Arc::clone(&approvals);
    let rt = Runtime::builder()
        .tool("act", |args| async move {
            Ok(json!(format!("acted:{}", args[0]["value"].as_str().unwrap())))
        })
        .approver(Arc::new(ProgrammaticApprover::new(move |_| {
            approvals_for_closure.fetch_add(1, Ordering::SeqCst);
            ApprovalDecision::Deny
        })))
        .build();

    let input = Value::Grounded(GroundedValue::with_confidence(
        Value::String(Arc::from("safe")),
        ProvenanceChain::new(),
        0.95,
    ));
    let value = run_agent(&ir, "run", vec![input], &rt)
        .await
        .expect("run");

    assert_eq!(value, Value::String(Arc::from("acted:safe")));
    assert_eq!(approvals.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prompt_confidence_flows_into_downstream_confidence_gate() {
    let src = r#"
effect low_confidence:
    confidence: 0.70

effect gated_action:
    trust: autonomous_if_confident(0.90)

prompt classify(input: String) -> String uses low_confidence:
    "Classify {input}."

tool act(label: String) -> String uses gated_action

@trust(autonomous)
agent run(input: String) -> String:
    label = classify(input)
    return act(label)
"#;
    let ir = ir_of(src);
    let approvals = Arc::new(AtomicUsize::new(0));
    let approvals_for_closure = Arc::clone(&approvals);
    let rt = Runtime::builder()
        .llm(Arc::new(MockAdapter::new("mock").reply("classify", json!("refund"))))
        .default_model("mock")
        .tool("act", |args| async move {
            Ok(json!(format!("acted:{}", args[0]["value"].as_str().unwrap())))
        })
        .approver(Arc::new(ProgrammaticApprover::new(move |req| {
            approvals_for_closure.fetch_add(1, Ordering::SeqCst);
            assert_eq!(req.label, "ConfidenceGate:act");
            ApprovalDecision::Approve
        })))
        .build();

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("ticket"))], &rt)
        .await
        .expect("run");

    assert_eq!(value, Value::String(Arc::from("acted:refund")));
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn step_trace_records_prompt_confidence_and_confidence_gate_threshold() {
    let src = r#"
effect low_confidence:
    confidence: 0.70

effect gated_action:
    trust: autonomous_if_confident(0.90)

prompt classify(input: String) -> String uses low_confidence:
    "Classify {input}."

tool act(label: String) -> String uses gated_action

@trust(autonomous)
agent run(input: String) -> String:
    label = classify(input)
    return act(label)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .llm(Arc::new(MockAdapter::new("mock").reply("classify", json!("refund"))))
        .default_model("mock")
        .tool("act", |args| async move {
            Ok(json!(format!("acted:{}", args[0]["value"].as_str().unwrap())))
        })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let hook = Arc::new(RecordingHook::new(Arc::new(BoundaryHook)));
    let trace_ref = hook.trace_ref();

    let (value, _) = run_agent_stepping(
        &ir,
        "run",
        vec![Value::String(Arc::from("ticket"))],
        &rt,
        hook,
        StepMode::Boundary,
    )
    .await
    .expect("run");
    assert_eq!(value, Value::String(Arc::from("acted:refund")));

    let trace = trace_ref.lock().unwrap().clone();
    let prompt_confidence = trace.checkpoints.iter().find_map(|checkpoint| {
        if let StepEvent::AfterPromptCall {
            prompt_name,
            result_confidence,
            ..
        } = &checkpoint.event
        {
            (prompt_name == "classify").then_some(*result_confidence)
        } else {
            None
        }
    });
    assert_eq!(prompt_confidence, Some(0.70));

    let gate = trace.checkpoints.iter().find_map(|checkpoint| {
        if let StepEvent::BeforeApproval {
            label,
            confidence_gate: Some(gate),
            ..
        } = &checkpoint.event
        {
            (label == "ConfidenceGate:act").then_some(*gate)
        } else {
            None
        }
    });
    let gate = gate.expect("confidence gate event");
    assert!(gate.triggered);
    assert!((gate.actual - 0.70).abs() < 1e-9);
    assert!((gate.threshold - 0.90).abs() < 1e-9);
}

#[tokio::test]
async fn calibrated_prompt_records_miscalibration_stats() {
    let src = r#"
effect confident_model:
    confidence: 0.90

prompt classify(input: String) -> String uses confident_model:
    calibrated
    "Classify {input}."

agent run(input: String) -> String:
    return classify(input)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .llm(Arc::new(MockAdapter::new("mock").reply_with_calibration(
            "classify",
            json!("wrong"),
            TokenUsage::default(),
            false,
        )))
        .default_model("mock")
        .build();

    for _ in 0..3 {
        let value = run_agent(&ir, "run", vec![Value::String(Arc::from("ticket"))], &rt)
            .await
            .expect("run");
        match value {
            Value::Grounded(g) => {
                assert_eq!(g.inner.get(), Value::String(Arc::from("wrong")));
                assert!((g.confidence - 0.90).abs() < 1e-9);
            }
            other => panic!("expected grounded confidence wrapper, got {other:?}"),
        }
    }

    let stats = rt
        .calibration_stats("classify", "mock")
        .expect("calibration stats");
    assert_eq!(stats.samples, 3);
    assert_eq!(stats.correct, 0);
    assert!((stats.mean_confidence - 0.90).abs() < 1e-9);
    assert!(stats.miscalibrated);
}

#[tokio::test]
async fn cacheable_prompt_reuses_response() {
    let src = "\
prompt classify(ctx: String) -> String:
    cacheable: true
    \"Classify {ctx}.\"

agent run(ctx: String) -> String:
    first = classify(ctx)
    second = classify(ctx)
    return second
";
    let ir = ir_of(src);
    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = CountingAdapter {
        model: "mock".into(),
        calls: calls.clone(),
        response: json!("refund"),
    };
    let rt = Runtime::builder()
        .default_model("mock")
        .llm(Arc::new(adapter))
        .build();

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("ticket"))], &rt)
        .await
        .expect("run");
    assert_eq!(value, Value::String(Arc::from("refund")));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "second identical cacheable prompt call must not hit the adapter"
    );
}

#[tokio::test]
async fn prompt_call_returns_struct_via_mock_adapter() {
    let src = r#"
type Decision:
    should_refund: Bool

prompt decide(reason: String) -> Decision:
    """Decide based on {reason}."""

agent run(reason: String) -> Decision:
    return decide(reason)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .llm(Arc::new(
            corvid_runtime::MockAdapter::new("mock-1")
                .reply("decide", json!({"should_refund": true})),
        ))
        .default_model("mock-1")
        .build();
    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("legit"))], &rt)
        .await
        .expect("run");
    match v {
        Value::Struct(s) => {
            assert_eq!(s.type_name(), "Decision");
            assert_eq!(s.get_field("should_refund").unwrap(), Value::Bool(true));
        }
        other => panic!("expected Decision struct, got {other:?}"),
    }
}

#[tokio::test]
async fn prompt_cites_strictly_accepts_response_with_context_phrase() {
    let src = r#"
effect retrieval:
    data: grounded

tool lookup(id: String) -> Grounded<String> uses retrieval

prompt answer(ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    "Answer from {ctx}"

agent run(id: String) -> Grounded<String>:
    ctx = lookup(id)
    return answer(ctx)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("lookup", |_args| async move {
            Ok(json!("alpha beta gamma delta epsilon"))
        })
        .llm(Arc::new(
            MockAdapter::new("mock-1")
                .reply("answer", json!("beta gamma delta epsilon")),
        ))
        .default_model("mock-1")
        .build();

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("doc-1"))], &rt)
        .await
        .expect("run");
    match value {
        Value::Grounded(g) => {
            assert_eq!(g.inner.get(), Value::String(Arc::from("beta gamma delta epsilon")));
        }
        other => panic!("expected grounded string, got {other:?}"),
    }
}

#[tokio::test]
async fn prompt_cites_strictly_rejects_response_without_context_phrase() {
    let src = r#"
effect retrieval:
    data: grounded

tool lookup(id: String) -> Grounded<String> uses retrieval

prompt answer(ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    "Answer from {ctx}"

agent run(id: String) -> Grounded<String>:
    ctx = lookup(id)
    return answer(ctx)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("lookup", |_args| async move {
            Ok(json!("alpha beta gamma delta epsilon"))
        })
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("answer", json!("unrelated answer")),
        ))
        .default_model("mock-1")
        .build();

    let err = run_agent(&ir, "run", vec![Value::String(Arc::from("doc-1"))], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::Other(message) => {
            assert!(message.contains("citation verification failed"), "{message}");
        }
        other => panic!("expected citation verification failure, got {other:?}"),
    }
}

#[tokio::test]
async fn capability_dispatch_chooses_cheapest_eligible_model_and_traces_it() {
    let src = r#"
model haiku:
    capability: basic

model opus:
    capability: expert
    version: "2024-10-22"

prompt answer(q: String) -> String:
    requires: expert
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-capability-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-capability"))
        .llm(Arc::new(MockAdapter::new("haiku").reply("answer", json!("cheap"))))
        .llm(Arc::new(MockAdapter::new("opus").reply("answer", json!("expert"))))
        .model(
            RegisteredModel::new("haiku")
                .capability("basic")
                .cost_per_token_in(0.00000025)
                .cost_per_token_out(0.00000125),
        )
        .model(
            RegisteredModel::new("opus")
                .capability("expert")
                .version("2024-10-22")
                .cost_per_token_in(0.000015)
                .cost_per_token_out(0.00002),
        )
        .default_model("haiku")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("hard"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("expert")));
    drop(rt);

    let trace_path = trace_dir.join("run-capability.jsonl");
    let body = std::fs::read_to_string(trace_path).expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ModelSelected {
            prompt,
            model,
            model_version,
            capability_required,
            capability_picked,
            ..
        } if prompt == "answer"
            && model == "opus"
            && model_version.as_deref() == Some("2024-10-22")
            && capability_required.as_deref() == Some("expert")
            && capability_picked.as_deref() == Some("expert")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall { prompt, model, model_version, .. }
            if prompt == "answer"
                && model.as_deref() == Some("opus")
                && model_version.as_deref() == Some("2024-10-22")
    )));
}

#[tokio::test]
async fn capability_dispatch_errors_when_no_model_qualifies() {
    let src = r#"
model haiku:
    capability: basic

prompt answer(q: String) -> String:
    requires: expert
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .llm(Arc::new(MockAdapter::new("haiku").reply("answer", json!("cheap"))))
        .model(
            RegisteredModel::new("haiku")
                .capability("basic")
                .cost_per_token_in(0.00000025),
        )
        .build();

    let err = run_agent(&ir, "run", vec![Value::String(Arc::from("hard"))], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::NoEligibleModel {
            required_capability,
            required_output_format,
            available_models,
        }) => {
            assert_eq!(required_capability, "expert");
            assert_eq!(required_output_format, None);
            assert_eq!(available_models, vec!["haiku".to_string()]);
        }
        other => panic!("expected Runtime(NoEligibleModel), got {other:?}"),
    }
}

#[tokio::test]
async fn output_format_dispatch_chooses_matching_model_and_traces_it() {
    let src = r#"
model loose:
    capability: expert
    output_format: markdown_strict

model jsoner:
    capability: basic
    output_format: strict_json

prompt answer(q: String) -> String:
    output_format: strict_json
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-output-format-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-output-format"))
        .llm(Arc::new(
            MockAdapter::new("loose").reply("answer", json!("markdown")),
        ))
        .llm(Arc::new(MockAdapter::new("jsoner").reply("answer", json!("json"))))
        .model(
            RegisteredModel::new("loose")
                .capability("expert")
                .output_format("markdown_strict")
                .cost_per_token_in(0.00000025)
                .cost_per_token_out(0.00000125),
        )
        .model(
            RegisteredModel::new("jsoner")
                .capability("basic")
                .output_format("strict_json")
                .cost_per_token_in(0.000015)
                .cost_per_token_out(0.00002),
        )
        .default_model("loose")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("hard"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("json")));
    drop(rt);

    let trace_path = trace_dir.join("run-output-format.jsonl");
    let body = std::fs::read_to_string(trace_path).expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ModelSelected {
            prompt,
            model,
            output_format_required,
            output_format_picked,
            ..
        } if prompt == "answer"
            && model == "jsoner"
            && output_format_required.as_deref() == Some("strict_json")
            && output_format_picked.as_deref() == Some("strict_json")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall { prompt, model, .. }
            if prompt == "answer" && model.as_deref() == Some("jsoner")
    )));
}

#[tokio::test]
async fn route_dispatch_selects_first_matching_arm_and_traces_it() {
    let src = r#"
model fast:
    capability: basic

model slow:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == "hard" -> slow
        _ -> fast
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-route-first-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-route-first"))
        .llm(Arc::new(MockAdapter::new("fast").reply("answer", json!("fast"))))
        .llm(Arc::new(MockAdapter::new("slow").reply("answer", json!("slow"))))
        .model(
            RegisteredModel::new("fast")
                .capability("basic")
                .cost_per_token_in(0.00000025)
                .cost_per_token_out(0.00000125),
        )
        .model(
            RegisteredModel::new("slow")
                .capability("expert")
                .cost_per_token_in(0.000015)
                .cost_per_token_out(0.00002),
        )
        .default_model("fast")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("hard"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("slow")));
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-route-first.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ModelSelected {
            prompt,
            model,
            arm_index,
            ..
        } if prompt == "answer" && model == "slow" && *arm_index == Some(0)
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall { prompt, model, .. }
            if prompt == "answer" && model.as_deref() == Some("slow")
    )));
}

#[tokio::test]
async fn route_dispatch_uses_wildcard_fallback_arm() {
    let src = r#"
model fast:
    capability: basic

model slow:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == "hard" -> slow
        _ -> fast
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-route-fallback-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-route-fallback"))
        .llm(Arc::new(MockAdapter::new("fast").reply("answer", json!("fast"))))
        .llm(Arc::new(MockAdapter::new("slow").reply("answer", json!("slow"))))
        .model(RegisteredModel::new("fast").capability("basic"))
        .model(RegisteredModel::new("slow").capability("expert"))
        .default_model("fast")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("easy"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("fast")));
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-route-fallback.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ModelSelected {
            prompt,
            model,
            arm_index,
            ..
        } if prompt == "answer" && model == "fast" && *arm_index == Some(1)
    )));
}

#[tokio::test]
async fn route_dispatch_errors_when_no_arm_matches() {
    let src = r#"
model slow:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == "hard" -> slow
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .llm(Arc::new(MockAdapter::new("slow").reply("answer", json!("slow"))))
        .default_model("slow")
        .build();

    let err = run_agent(&ir, "run", vec![Value::String(Arc::from("easy"))], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::NoMatchingRoute { prompt }) => {
            assert_eq!(prompt, "answer");
        }
        other => panic!("expected Runtime(NoMatchingRoute), got {other:?}"),
    }
}

#[tokio::test]
async fn route_dispatch_can_call_prompt_guards() {
    let src = r#"
model classifier:
    capability: basic

model fast:
    capability: basic

model slow:
    capability: expert

prompt is_hard(q: String) -> Bool:
    "Is {q} hard?"

prompt answer(q: String) -> String:
    route:
        is_hard(q) -> slow
        _ -> fast
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-route-guard-prompt-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-route-guard-prompt"))
        .llm(Arc::new(MockAdapter::new("classifier").reply("is_hard", json!(true))))
        .llm(Arc::new(MockAdapter::new("fast").reply("answer", json!("fast"))))
        .llm(Arc::new(MockAdapter::new("slow").reply("answer", json!("slow"))))
        .model(RegisteredModel::new("fast").capability("basic"))
        .model(RegisteredModel::new("slow").capability("expert"))
        .default_model("classifier")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("essay"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("slow")));
    drop(rt);

    let body = std::fs::read_to_string(
        trace_dir.join("run-route-guard-prompt.jsonl"),
    )
    .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall { prompt, model, .. }
            if prompt == "is_hard" && model.as_deref() == Some("classifier")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ModelSelected {
            prompt,
            model,
            arm_index,
            ..
        } if prompt == "answer" && model == "slow" && *arm_index == Some(0)
    )));
}

#[tokio::test]
async fn progressive_dispatch_returns_stage_zero_when_confidence_is_high() {
    let src = r#"
model cheap:
    capability: basic

model expensive:
    capability: expert

prompt answer(q: String) -> String:
    progressive:
        cheap below 0.95
        expensive
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-progressive-high-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-progressive-high"))
        .llm(Arc::new(MockAdapter::new("cheap").reply("answer", json!("cheap"))))
        .model(RegisteredModel::new("cheap").capability("basic"))
        .model(RegisteredModel::new("expensive").capability("expert"))
        .default_model("cheap")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("easy"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("cheap")));
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-progressive-high.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    let llm_calls = events
        .iter()
        .filter(|event| matches!(event, TraceEvent::LlmCall { prompt, .. } if prompt == "answer"))
        .count();
    assert_eq!(llm_calls, 1);
    assert!(!events.iter().any(|event| matches!(event, TraceEvent::ProgressiveEscalation { .. })));
    assert!(!events.iter().any(|event| matches!(event, TraceEvent::ProgressiveExhausted { .. })));
}

#[tokio::test]
async fn progressive_dispatch_exhausts_all_stages_when_confidence_stays_low() {
    let src = r#"
model cheap:
    capability: basic

model medium:
    capability: standard

model expensive:
    capability: expert

prompt answer(q: String) -> String:
    progressive:
        cheap below 0.95
        medium below 0.99
        expensive
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-progressive-exhausted-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-progressive-exhausted"))
        .llm(Arc::new(MockAdapter::new("cheap").reply("answer", json!("cheap"))))
        .llm(Arc::new(MockAdapter::new("medium").reply("answer", json!("medium"))))
        .llm(Arc::new(MockAdapter::new("expensive").reply("answer", json!("expensive"))))
        .model(RegisteredModel::new("cheap").capability("basic"))
        .model(RegisteredModel::new("medium").capability("standard"))
        .model(RegisteredModel::new("expensive").capability("expert"))
        .default_model("cheap")
        .build();
    let grounded = Value::Grounded(GroundedValue::with_confidence(
        Value::String(Arc::from("hard")),
        ProvenanceChain::new(),
        0.40,
    ));

    let v = run_agent(&ir, "run", vec![grounded], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::Grounded(GroundedValue::new(
        Value::String(Arc::from("expensive")),
        ProvenanceChain::new(),
    )));
    drop(rt);

    let body = std::fs::read_to_string(
        trace_dir.join("run-progressive-exhausted.jsonl"),
    )
    .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    let llm_calls = events
        .iter()
        .filter(|event| matches!(event, TraceEvent::LlmCall { prompt, .. } if prompt == "answer"))
        .count();
    assert_eq!(llm_calls, 3);
    let escalations: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            TraceEvent::ProgressiveEscalation {
                from_stage,
                to_stage,
                confidence_observed,
                threshold,
                ..
            } => Some((*from_stage, *to_stage, *confidence_observed, *threshold)),
            _ => None,
        })
        .collect();
    assert_eq!(escalations.len(), 2);
    assert_eq!(escalations[0].0, 0);
    assert_eq!(escalations[0].1, 1);
    assert!((escalations[0].2 - 0.40).abs() < 1e-9);
    assert!((escalations[0].3 - 0.95).abs() < 1e-9);
    assert_eq!(escalations[1].0, 1);
    assert_eq!(escalations[1].1, 2);
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ProgressiveExhausted { prompt, stages, .. }
            if prompt == "answer"
                && stages == &vec![
                    "cheap".to_string(),
                    "medium".to_string(),
                    "expensive".to_string(),
                ]
    )));
}

#[tokio::test]
async fn progressive_dispatch_halts_when_budget_is_exceeded_mid_chain() {
    let src = r#"
model cheap:
    capability: basic

model expensive:
    capability: expert

prompt answer(q: String) -> String:
    progressive:
        cheap below 0.95
        expensive
    "Answer {q}."

@budget($0.50)
agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-progressive-budget-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-progressive-budget"))
        .llm(Arc::new(MockAdapter::new("cheap").reply_with_usage(
            "answer",
            json!("cheap"),
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        )))
        .llm(Arc::new(MockAdapter::new("expensive").reply_with_usage(
            "answer",
            json!("expensive"),
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        )))
        .model(
            RegisteredModel::new("cheap")
                .capability("basic")
                .cost_per_token_in(0.15)
                .cost_per_token_out(0.15),
        )
        .model(
            RegisteredModel::new("expensive")
                .capability("expert")
                .cost_per_token_in(0.15)
                .cost_per_token_out(0.15),
        )
        .default_model("cheap")
        .build();
    let grounded = Value::Grounded(GroundedValue::with_confidence(
        Value::String(Arc::from("hard")),
        ProvenanceChain::new(),
        0.40,
    ));

    let err = run_agent(&ir, "run", vec![grounded], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::BudgetExceeded { budget, used } => {
            assert!((budget - 0.50).abs() < 1e-9);
            assert!((used - 0.60).abs() < 1e-9);
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }
    drop(rt);

    let body = std::fs::read_to_string(
        trace_dir.join("run-progressive-budget.jsonl"),
    )
    .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ProgressiveEscalation { from_stage, to_stage, .. }
            if *from_stage == 0 && *to_stage == 1
    )));
    assert!(!events.iter().any(|event| matches!(event, TraceEvent::ProgressiveExhausted { .. })));
}

#[tokio::test]
async fn rollout_dispatch_zero_percent_always_uses_baseline() {
    let src = r#"
model baseline:
    capability: basic

model variant:
    capability: expert

prompt answer(q: String) -> String:
    rollout 0% variant, else baseline
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-rollout-zero-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-rollout-zero"))
        .llm(Arc::new(MockAdapter::new("baseline").reply("answer", json!("baseline"))))
        .llm(Arc::new(MockAdapter::new("variant").reply("answer", json!("variant"))))
        .model(RegisteredModel::new("baseline").capability("basic"))
        .model(RegisteredModel::new("variant").capability("expert"))
        .rollout_seed(42)
        .default_model("baseline")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("x"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("baseline")));
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-rollout-zero.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::AbVariantChosen {
            prompt,
            variant,
            baseline,
            rollout_pct,
            chosen,
            ..
        } if prompt == "answer"
            && variant == "variant"
            && baseline == "baseline"
            && (*rollout_pct - 0.0).abs() < 1e-9
            && chosen == "baseline"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall { prompt, model, .. }
            if prompt == "answer" && model.as_deref() == Some("baseline")
    )));
}

#[tokio::test]
async fn rollout_dispatch_hundred_percent_always_uses_variant() {
    let src = r#"
model baseline:
    capability: basic

model variant:
    capability: expert

prompt answer(q: String) -> String:
    rollout 100% variant, else baseline
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-rollout-hundred-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-rollout-hundred"))
        .llm(Arc::new(MockAdapter::new("baseline").reply("answer", json!("baseline"))))
        .llm(Arc::new(MockAdapter::new("variant").reply("answer", json!("variant"))))
        .model(RegisteredModel::new("baseline").capability("basic"))
        .model(RegisteredModel::new("variant").capability("expert"))
        .rollout_seed(42)
        .default_model("baseline")
        .build();

    let v = run_agent(&ir, "run", vec![Value::String(Arc::from("x"))], &rt)
        .await
        .expect("run");
    assert_eq!(v, Value::String(Arc::from("variant")));
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-rollout-hundred.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::AbVariantChosen {
            rollout_pct,
            chosen,
            ..
        } if (*rollout_pct - 100.0).abs() < 1e-9 && chosen == "variant"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall { prompt, model, .. }
            if prompt == "answer" && model.as_deref() == Some("variant")
    )));
}

#[tokio::test]
async fn rollout_dispatch_is_stable_for_same_seed_across_restarts() {
    let src = r#"
model baseline:
    capability: basic

model variant:
    capability: expert

prompt answer(q: String) -> String:
    rollout 35% variant, else baseline
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);

    async fn run_sequence(
        ir: &corvid_ir::IrFile,
        seed: u64,
    ) -> Vec<String> {
        let rt = Runtime::builder()
            .llm(Arc::new(MockAdapter::new("baseline").reply("answer", json!("baseline"))))
            .llm(Arc::new(MockAdapter::new("variant").reply("answer", json!("variant"))))
            .model(RegisteredModel::new("baseline").capability("basic"))
            .model(RegisteredModel::new("variant").capability("expert"))
            .rollout_seed(seed)
            .default_model("baseline")
            .build();

        let mut out = Vec::new();
        for _ in 0..8 {
            let value = run_agent(ir, "run", vec![Value::String(Arc::from("x"))], &rt)
                .await
                .expect("run");
            match value {
                Value::String(s) => out.push(s.to_string()),
                other => panic!("expected string result, got {other:?}"),
            }
        }
        out
    }

    let first = run_sequence(&ir, 1337).await;
    let second = run_sequence(&ir, 1337).await;
    assert_eq!(first, second);
}

#[tokio::test]
async fn ensemble_dispatch_uses_majority_winner_and_emits_vote() {
    let src = r#"
model a:
    capability: basic

model b:
    capability: standard

model c:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [a, b, c] vote majority
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-ensemble-majority-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-ensemble-majority"))
        .llm(Arc::new(MockAdapter::new("a").reply("answer", json!("alpha"))))
        .llm(Arc::new(MockAdapter::new("b").reply("answer", json!("alpha"))))
        .llm(Arc::new(MockAdapter::new("c").reply("answer", json!("beta"))))
        .model(RegisteredModel::new("a").capability("basic"))
        .model(RegisteredModel::new("b").capability("standard"))
        .model(RegisteredModel::new("c").capability("expert"))
        .default_model("a")
        .build();

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("x"))], &rt)
        .await
        .expect("run");
    match value {
        Value::Grounded(g) => {
            assert_eq!(g.inner.get(), Value::String(Arc::from("alpha")));
            assert!((g.confidence - (2.0 / 3.0)).abs() < 1e-9);
        }
        other => panic!("expected grounded majority winner, got {other:?}"),
    }
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-ensemble-majority.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::EnsembleVote {
            prompt,
            members,
            results,
            winner,
            agreement_rate,
            strategy,
            ..
        } if prompt == "answer"
            && members == &vec!["a".to_string(), "b".to_string(), "c".to_string()]
            && results == &vec!["alpha".to_string(), "alpha".to_string(), "beta".to_string()]
            && winner == "alpha"
            && (*agreement_rate - (2.0 / 3.0)).abs() < 1e-9
            && strategy == "majority"
    )));
}

#[tokio::test]
async fn ensemble_dispatch_breaks_ties_alphabetically() {
    let src = r#"
model a:
    capability: basic

model b:
    capability: standard

prompt answer(q: String) -> String:
    ensemble [a, b] vote majority
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .llm(Arc::new(MockAdapter::new("a").reply("answer", json!("zulu"))))
        .llm(Arc::new(MockAdapter::new("b").reply("answer", json!("alpha"))))
        .model(RegisteredModel::new("a").capability("basic"))
        .model(RegisteredModel::new("b").capability("standard"))
        .default_model("a")
        .build();

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("x"))], &rt)
        .await
        .expect("run");
    match value {
        Value::Grounded(g) => {
            assert_eq!(g.inner.get(), Value::String(Arc::from("alpha")));
            assert!((g.confidence - 0.5).abs() < 1e-9);
        }
        other => panic!("expected grounded alphabetical tie-break winner, got {other:?}"),
    }
}

#[tokio::test]
async fn ensemble_weighted_by_accuracy_history_can_override_raw_majority() {
    let src = r#"
model a:
    capability: basic

model b:
    capability: standard

model c:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [a, b, c] vote majority weighted_by accuracy_history
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-ensemble-weighted-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-ensemble-weighted"))
        .llm(Arc::new(MockAdapter::new("a").reply("answer", json!("alpha"))))
        .llm(Arc::new(MockAdapter::new("b").reply("answer", json!("beta"))))
        .llm(Arc::new(MockAdapter::new("c").reply("answer", json!("beta"))))
        .model(RegisteredModel::new("a").capability("basic"))
        .model(RegisteredModel::new("b").capability("standard"))
        .model(RegisteredModel::new("c").capability("expert"))
        .default_model("a")
        .build();
    rt.record_calibration("answer", "a", 1.0, true);
    rt.record_calibration("answer", "b", 1.0, false);
    rt.record_calibration("answer", "c", 1.0, false);

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("x"))], &rt)
        .await
        .expect("run");
    assert_eq!(value, Value::String(Arc::from("alpha")));
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-ensemble-weighted.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::EnsembleVote {
            prompt,
            winner,
            strategy,
            weights,
            escalated_to,
            ..
        } if prompt == "answer"
            && winner == "alpha"
            && strategy == "majority weighted_by accuracy_history"
            && weights.as_ref() == Some(&vec![1.0, 0.0, 0.0])
            && escalated_to.is_none()
    )));
}

#[tokio::test]
async fn ensemble_disagreement_escalates_to_configured_model() {
    let src = r#"
model a:
    capability: basic

model b:
    capability: standard

model judge:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [a, b] vote majority on disagreement escalate_to judge
    "Answer {q}."

agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-ensemble-escalation-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(
            &trace_dir,
            "run-ensemble-escalation",
        ))
        .llm(Arc::new(MockAdapter::new("a").reply("answer", json!("alpha"))))
        .llm(Arc::new(MockAdapter::new("b").reply("answer", json!("beta"))))
        .llm(Arc::new(MockAdapter::new("judge").reply("answer", json!("final"))))
        .model(RegisteredModel::new("a").capability("basic"))
        .model(RegisteredModel::new("b").capability("standard"))
        .model(RegisteredModel::new("judge").capability("expert"))
        .default_model("a")
        .build();

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("x"))], &rt)
        .await
        .expect("run");
    assert_eq!(value, Value::String(Arc::from("final")));
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-ensemble-escalation.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::EnsembleVote {
            prompt,
            agreement_rate,
            escalated_to,
            ..
        } if prompt == "answer"
            && (*agreement_rate - 0.5).abs() < 1e-9
            && escalated_to.as_deref() == Some("judge")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall { prompt, model, .. }
            if prompt == "answer" && model.as_deref() == Some("judge")
    )));
}

#[tokio::test]
async fn ensemble_dispatch_charges_sum_of_member_costs() {
    let src = r#"
model a:
    capability: basic

model b:
    capability: standard

model c:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [a, b, c] vote majority
    "Answer {q}."

@budget($0.50)
agent run(q: String) -> String:
    return answer(q)
"#;
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .llm(Arc::new(MockAdapter::new("a").reply_with_usage(
            "answer",
            json!("alpha"),
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        )))
        .llm(Arc::new(MockAdapter::new("b").reply_with_usage(
            "answer",
            json!("alpha"),
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        )))
        .llm(Arc::new(MockAdapter::new("c").reply_with_usage(
            "answer",
            json!("beta"),
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        )))
        .model(
            RegisteredModel::new("a")
                .capability("basic")
                .cost_per_token_in(0.1)
                .cost_per_token_out(0.1),
        )
        .model(
            RegisteredModel::new("b")
                .capability("standard")
                .cost_per_token_in(0.1)
                .cost_per_token_out(0.1),
        )
        .model(
            RegisteredModel::new("c")
                .capability("expert")
                .cost_per_token_in(0.1)
                .cost_per_token_out(0.1),
        )
        .default_model("a")
        .build();

    let err = run_agent(&ir, "run", vec![Value::String(Arc::from("x"))], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::BudgetExceeded { budget, used } => {
            assert!((budget - 0.50).abs() < 1e-9);
            assert!((used - 0.60).abs() < 1e-9);
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }
}

#[tokio::test]
async fn adversarial_pipeline_returns_adjudicator_result_and_emits_completion() {
    let src = r#"
type Verdict:
    contradiction: Bool
    rationale: String

prompt propose_answer(q: String) -> String:
    "Answer: {q}"

prompt critique(proposed: String) -> String:
    "Flaws in: {proposed}"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    "Verdict"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    "Verify"

agent run(q: String) -> Verdict:
    return verify(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-adversarial-ok-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-adversarial-ok"))
        .llm(Arc::new(
            MockAdapter::new("mock-1")
                .reply("propose_answer", json!("draft"))
                .reply("critique", json!("needs citations"))
                .reply(
                    "adjudicate_fn",
                    json!({"contradiction": false, "rationale": "accepted"}),
                ),
        ))
        .default_model("mock-1")
        .build();

    let value = run_agent(&ir, "run", vec![Value::String(Arc::from("q"))], &rt)
        .await
        .expect("run");
    match value {
        Value::Struct(s) => {
            assert_eq!(s.type_name(), "Verdict");
            assert_eq!(s.get_field("contradiction").unwrap(), Value::Bool(false));
            assert_eq!(
                s.get_field("rationale").unwrap(),
                Value::String(Arc::from("accepted"))
            );
        }
        other => panic!("expected Verdict struct, got {other:?}"),
    }
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-adversarial-ok.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, TraceEvent::LlmCall { .. }))
            .count(),
        3
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::AdversarialPipelineCompleted {
            prompt,
            contradiction,
            ..
        } if prompt == "verify" && !contradiction
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        TraceEvent::AdversarialContradiction { .. }
    )));
}

#[tokio::test]
async fn adversarial_pipeline_emits_contradiction_event_when_flagged() {
    let src = r#"
type Verdict:
    contradiction: Bool
    rationale: String

prompt propose_answer(q: String) -> String:
    "Answer: {q}"

prompt critique(proposed: String) -> String:
    "Flaws in: {proposed}"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    "Verdict"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    "Verify"

agent run(q: String) -> Verdict:
    return verify(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-adversarial-contradiction-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(
            &trace_dir,
            "run-adversarial-contradiction",
        ))
        .llm(Arc::new(
            MockAdapter::new("mock-1")
                .reply("propose_answer", json!("draft"))
                .reply("critique", json!("fatal flaw"))
                .reply(
                    "adjudicate_fn",
                    json!({"contradiction": true, "rationale": "reject"}),
                ),
        ))
        .default_model("mock-1")
        .build();

    run_agent(&ir, "run", vec![Value::String(Arc::from("q"))], &rt)
        .await
        .expect("run");
    drop(rt);

    let body = std::fs::read_to_string(
        trace_dir.join("run-adversarial-contradiction.jsonl"),
    )
    .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::AdversarialContradiction {
            prompt,
            proposed,
            challenge,
            verdict,
            ..
        } if prompt == "verify"
            && proposed == "draft"
            && challenge == "fatal flaw"
            && verdict == &json!({"contradiction": true, "rationale": "reject"})
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::AdversarialPipelineCompleted {
            prompt,
            contradiction,
            ..
        } if prompt == "verify" && *contradiction
    )));
}

#[tokio::test]
async fn adversarial_pipeline_halts_on_budget_before_adjudicator() {
    let src = r#"
type Verdict:
    contradiction: Bool
    rationale: String

prompt propose_answer(q: String) -> String:
    "Answer: {q}"

prompt critique(proposed: String) -> String:
    "Flaws in: {proposed}"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    "Verdict"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    "Verify"

@budget($0.50)
agent run(q: String) -> Verdict:
    return verify(q)
"#;
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-adversarial-budget-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(
            &trace_dir,
            "run-adversarial-budget",
        ))
        .llm(Arc::new(
            MockAdapter::new("mock-1")
                .reply_with_usage(
                    "propose_answer",
                    json!("draft"),
                    TokenUsage {
                        prompt_tokens: 1,
                        completion_tokens: 1,
                        total_tokens: 2,
                    },
                )
                .reply_with_usage(
                    "critique",
                    json!("fatal flaw"),
                    TokenUsage {
                        prompt_tokens: 1,
                        completion_tokens: 1,
                        total_tokens: 2,
                    },
                )
                .reply_with_usage(
                    "adjudicate_fn",
                    json!({"contradiction": true, "rationale": "reject"}),
                    TokenUsage {
                        prompt_tokens: 1,
                        completion_tokens: 1,
                        total_tokens: 2,
                    },
                ),
        ))
        .model(
            RegisteredModel::new("mock-1")
                .capability("basic")
                .cost_per_token_in(0.15)
                .cost_per_token_out(0.15),
        )
        .default_model("mock-1")
        .build();

    let err = run_agent(&ir, "run", vec![Value::String(Arc::from("q"))], &rt)
        .await
        .unwrap_err();
    match err.kind {
        InterpErrorKind::BudgetExceeded { budget, used } => {
            assert!((budget - 0.50).abs() < 1e-9);
            assert!((used - 0.60).abs() < 1e-9);
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }
    drop(rt);

    let body = std::fs::read_to_string(trace_dir.join("run-adversarial-budget.jsonl"))
        .expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    let llm_calls: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            TraceEvent::LlmCall { prompt, .. } => Some(prompt.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(llm_calls, vec!["propose_answer".to_string(), "critique".to_string()]);
    assert!(!events.iter().any(|event| matches!(
        event,
        TraceEvent::AdversarialPipelineCompleted { .. }
    )));
}

#[tokio::test]
async fn agent_to_agent_call_recurses() {
    let src = "\
agent inner(n: Int) -> Int:
    return n + 1

agent outer(n: Int) -> Int:
    return inner(n) + inner(n)
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "outer", vec![Value::Int(10)], &rt).await.expect("run");
    assert_eq!(v, Value::Int(22));
}

#[tokio::test]
async fn span_is_preserved_in_errors() {
    let ir = ir_of("agent bad() -> Int:\n    return 10 / 0\n");
    let rt = empty_runtime();
    let err = run_agent(&ir, "bad", vec![], &rt).await.unwrap_err();
    assert_ne!(err.span, Span::new(0, 0));
}
