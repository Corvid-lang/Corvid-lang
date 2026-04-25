use super::*;

#[tokio::test]
async fn stream_agent_yields_values_over_mpsc() {
    let src = "\
agent chunks(text: String) -> Stream<String>:
    yield text
    yield text + \"!\"
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let stream = run_agent(&ir, "chunks", vec![Value::String(Arc::from("hi"))], &rt)
        .await
        .expect("run");
    let items = collect_stream(stream).await.expect("collect");
    assert_eq!(
        items,
        vec![
            Value::String(Arc::from("hi")),
            Value::String(Arc::from("hi!")),
        ]
    );
}

#[tokio::test]
async fn stream_grounded_elements_update_aggregate_provenance_as_consumed() {
    let src = "\
effect retrieval:
    data: grounded

tool fetch_a() -> Grounded<String> uses retrieval
tool fetch_b() -> Grounded<String> uses retrieval

agent docs() -> Stream<Grounded<String>>:
    yield fetch_a()
    yield fetch_b()
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .tool("fetch_a", |_| async move { Ok(json!("a")) })
        .tool("fetch_b", |_| async move { Ok(json!("b")) })
        .build();
    let value = run_agent(&ir, "docs", vec![], &rt).await.expect("run");
    let Value::Stream(stream) = value else {
        panic!("expected stream");
    };

    assert!(stream.provenance().entries.is_empty());

    let first = stream
        .next()
        .await
        .expect("first")
        .expect("first ok");
    match first {
        Value::Grounded(g) => {
            assert_eq!(g.inner.get(), Value::String(Arc::from("a")));
            assert!(g.provenance.has_source("fetch_a"));
        }
        other => panic!("expected grounded first element, got {other:?}"),
    }
    let first_sources = stream.provenance();
    assert!(first_sources.has_source("fetch_a"));
    assert!(!first_sources.has_source("fetch_b"));
    assert!(render_value(&Value::Stream(stream.clone())).contains("retrieval:fetch_a"));

    let second = stream
        .next()
        .await
        .expect("second")
        .expect("second ok");
    match second {
        Value::Grounded(g) => {
            assert_eq!(g.inner.get(), Value::String(Arc::from("b")));
            assert!(g.provenance.has_source("fetch_b"));
        }
        other => panic!("expected grounded second element, got {other:?}"),
    }
    let all_sources = stream.provenance();
    assert!(all_sources.has_source("fetch_a"));
    assert!(all_sources.has_source("fetch_b"));
}

#[tokio::test]
async fn for_loop_consumes_stream_values() {
    let src = "\
agent source() -> Stream<Int>:
    yield 1
    yield 2
    yield 3

agent sum_stream() -> Int:
    total = 0
    for x in source():
        total = total + x
    return total
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let value = run_agent(&ir, "sum_stream", vec![], &rt).await.expect("run");
    assert_eq!(value, Value::Int(6));
}

#[tokio::test]
async fn stream_prompt_is_wrapped_as_singleton_stream() {
    let src = "\
prompt generate(ctx: String) -> Stream<String>:
    with backpressure unbounded
    \"Generate {ctx}\"

agent relay(ctx: String) -> Stream<String>:
    for chunk in generate(ctx):
        yield chunk
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply("generate", json!("hello"))))
        .default_model("mock-1")
        .build();
    let stream = run_agent(&ir, "relay", vec![Value::String(Arc::from("ctx"))], &rt)
        .await
        .expect("run");
    let items = collect_stream(stream).await.expect("collect");
    assert_eq!(items, vec![Value::String(Arc::from("hello"))]);
}

#[tokio::test]
async fn resume_token_reopens_prompt_stream_with_delivered_context() {
    let src = "\
prompt draft(topic: String) -> Stream<String>:
    \"Draft {topic}\"

agent capture(topic: String) -> ResumeToken<String>:
    stream = draft(topic)
    for chunk in stream:
        break
    return resume_token(stream)

agent continue_it(token: ResumeToken<String>) -> String:
    stream = resume(draft, token)
    for chunk in stream:
        return chunk
    return \"empty\"
";
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-stream-resume-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let mock = Arc::new(MockAdapter::new("mock-1").reply("draft", json!("first")));
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-stream-resume"))
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(mock.clone())
        .default_model("mock-1")
        .build();

    let token = run_agent(&ir, "capture", vec![Value::String(Arc::from("launch"))], &rt)
        .await
        .expect("capture");
    assert!(matches!(token, Value::ResumeToken(_)));

    mock.add_reply("draft", json!("continued"));
    let value = run_agent(&ir, "continue_it", vec![token], &rt)
        .await
        .expect("continue");
    assert_eq!(value, Value::String(Arc::from("continued")));
    drop(rt);

    let body =
        std::fs::read_to_string(trace_dir.join("run-stream-resume.jsonl")).expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall {
            prompt,
            rendered: Some(rendered),
            ..
        } if prompt == "draft"
            && rendered.contains("Resume after delivered elements")
            && rendered.contains("first")
    )));
}

#[tokio::test]
async fn split_by_partitions_stream_and_merge_fair_round_robins_groups() {
    let src = "\
type Event:
    kind: String
    body: String

agent source() -> Stream<Event>:
    yield Event(\"b\", \"two\")
    yield Event(\"a\", \"one\")
    yield Event(\"b\", \"three\")

agent fanout() -> Stream<Event>:
    groups = source().split_by(\"kind\")
    for event in merge(groups).ordered_by(\"fair_round_robin\"):
        yield event
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let stream = run_agent(&ir, "fanout", vec![], &rt).await.expect("run");
    let items = collect_stream(stream).await.expect("collect");
    let bodies = items
        .iter()
        .map(|item| struct_string_field(item, "body"))
        .collect::<Vec<_>>();
    assert_eq!(bodies, vec!["two", "one", "three"]);
}

#[tokio::test]
async fn stream_prompt_confidence_floor_breaches_mid_stream() {
    let src = "\
effect shaky:
    confidence: 0.40

prompt generate(ctx: String) -> Stream<String> uses shaky:
    with min_confidence 0.80
    \"Generate {ctx}\"

agent relay(ctx: String) -> Stream<String>:
    for chunk in generate(ctx):
        yield chunk
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply("generate", json!("hello"))))
        .default_model("mock-1")
        .build();
    let stream = run_agent(&ir, "relay", vec![Value::String(Arc::from("ctx"))], &rt)
        .await
        .expect("run");
    let err = collect_stream(stream).await.unwrap_err();
    assert!(matches!(
        err.kind,
        InterpErrorKind::ConfidenceFloorBreached { floor, actual }
            if (floor - 0.80).abs() < 0.001 && (actual - 0.40).abs() < 0.001
    ));
}

#[tokio::test]
async fn stream_prompt_escalates_to_stronger_model_on_low_confidence() {
    let src = "\
model expert:
    capability: expert

prompt generate(ctx: String) -> Stream<String>:
    with min_confidence 0.80
    with escalate_to expert
    \"Generate {ctx}\"

agent relay(ctx: String) -> Stream<String>:
    for chunk in generate(ctx):
        yield chunk
";
    let ir = ir_of(src);
    let trace_dir = std::env::temp_dir().join(format!(
        "corvid-vm-stream-upgrade-{}",
        corvid_runtime::now_ms()
    ));
    std::fs::create_dir_all(&trace_dir).unwrap();
    let rt = Runtime::builder()
        .tracer(corvid_runtime::Tracer::open(&trace_dir, "run-stream-upgrade"))
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("cheap").reply_with_confidence(
            "generate",
            json!("draft"),
            TokenUsage::default(),
            0.40,
        )))
        .llm(Arc::new(MockAdapter::new("expert").reply_with_confidence(
            "generate",
            json!("final"),
            TokenUsage::default(),
            0.95,
        )))
        .model(RegisteredModel::new("expert").capability("expert"))
        .default_model("cheap")
        .build();
    let stream = run_agent(&ir, "relay", vec![Value::String(Arc::from("ctx"))], &rt)
        .await
        .expect("run");
    let items = collect_stream(stream).await.expect("collect");
    assert_eq!(items.len(), 1);
    match &items[0] {
        Value::Grounded(g) => {
            assert_eq!(g.inner.get(), Value::String(Arc::from("final")));
            assert!((g.confidence - 0.95).abs() < 0.001);
        }
        other => panic!("expected confidence-wrapped stream item, got {other:?}"),
    }
    drop(rt);

    let body =
        std::fs::read_to_string(trace_dir.join("run-stream-upgrade.jsonl")).expect("trace file");
    let events: Vec<TraceEvent> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid trace event"))
        .collect();

    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::StreamUpgrade {
            prompt,
            to_model,
            confidence_observed,
            threshold,
            partial,
            ..
        } if prompt == "generate"
            && to_model == "expert"
            && (*confidence_observed - 0.40).abs() < 0.001
            && (*threshold - 0.80).abs() < 0.001
            && partial.get("value") == Some(&json!("draft"))
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::LlmCall {
            prompt,
            model: Some(model),
            rendered: Some(rendered),
            ..
        } if prompt == "generate"
            && model == "expert"
            && rendered.contains("Continue from partial output")
            && rendered.contains("draft")
    )));
}

#[tokio::test]
async fn stream_partial_struct_fields_are_available_as_they_complete() {
    let src = "\
type Plan:
    title: String
    body: String

prompt plan(topic: String) -> Stream<Partial<Plan>>:
    \"Plan {topic}\"

agent first_title(topic: String) -> Option<String>:
    for snapshot in plan(topic):
        return snapshot.title
    return None
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply(
            "plan",
            json!({
                "title": { "tag": "complete", "value": "Ship Corvid" },
                "body": { "tag": "streaming" }
            }),
        )))
        .default_model("mock-1")
        .build();
    let value = run_agent(&ir, "first_title", vec![Value::String(Arc::from("launch"))], &rt)
        .await
        .expect("run");
    assert_eq!(
        value,
        Value::OptionSome(crate::value::BoxedValue::new(Value::String(Arc::from(
            "Ship Corvid"
        ))))
    );
}

#[tokio::test]
async fn stream_prompt_token_limit_breaches_mid_stream() {
    let src = "\
prompt generate(ctx: String) -> Stream<String>:
    with max_tokens 10
    \"Generate {ctx}\"

agent relay(ctx: String) -> Stream<String>:
    for chunk in generate(ctx):
        yield chunk
";
    let ir = ir_of(src);
    let rt = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply_with_usage(
                "generate",
                json!("hello"),
                TokenUsage {
                    prompt_tokens: 4,
                    completion_tokens: 25,
                    total_tokens: 29,
                },
            ),
        ))
        .default_model("mock-1")
        .build();
    let stream = run_agent(&ir, "relay", vec![Value::String(Arc::from("ctx"))], &rt)
        .await
        .expect("run");
    let err = collect_stream(stream).await.unwrap_err();
    assert!(matches!(
        err.kind,
        InterpErrorKind::TokenLimitExceeded { limit: 10, used: 25 }
    ));
}

#[tokio::test]
async fn stream_budget_termination_fires_before_over_budget_yield() {
    use corvid_ir::{
        IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrParam, IrPrompt,
        IrStmt,
    };
    use corvid_resolve::{DefId, LocalId};
    use corvid_types::Type;

    let sp = Span::new(0, 0);
    let prompt_id = DefId(1);
    let loop_local = LocalId(0);
    let prompt_call = IrExpr {
        kind: IrExprKind::Call {
            kind: IrCallKind::Prompt { def_id: prompt_id },
            callee_name: "generate".into(),
            args: vec![],
        },
        ty: Type::Stream(Box::new(Type::String)),
        span: sp,
    };
    let yielded_local = IrExpr {
        kind: IrExprKind::Local {
            local_id: loop_local,
            name: "chunk".into(),
        },
        ty: Type::String,
        span: sp,
    };
    let ir = IrFile {
        imports: vec![],
        types: vec![],
        tools: vec![],
        prompts: vec![IrPrompt {
            id: prompt_id,
            name: "generate".into(),
            params: vec![],
            return_ty: Type::Stream(Box::new(Type::String)),
            template: "Generate".into(),
            effect_names: vec!["expensive".into()],
            effect_cost: 0.75,
            effect_confidence: 1.0,
            cites_strictly_param: None,
            min_confidence: None,
            max_tokens: None,
            backpressure: Some(BackpressurePolicy::Bounded(1)),
            escalate_to: None,
            calibrated: false,
            capability_required: None,
            route: Vec::new(),
            progressive: Vec::new(),
            rollout: None,
            ensemble: None,
            adversarial: None,
            span: sp,
        }],
        agents: vec![IrAgent {
            id: DefId(2),
            name: "relay".into(),
            extern_abi: None,
            params: vec![IrParam {
                name: "ctx".into(),
                local_id: LocalId(1),
                ty: Type::String,
                span: sp,
            }],
            return_ty: Type::Stream(Box::new(Type::String)),
            cost_budget: Some(0.50),
            wrapping_arithmetic: false,
            body: IrBlock {
                stmts: vec![IrStmt::For {
                    var_local: loop_local,
                    var_name: "chunk".into(),
                    iter: prompt_call,
                    body: IrBlock {
                        stmts: vec![IrStmt::Yield {
                            value: yielded_local,
                            span: sp,
                        }],
                        span: sp,
                    },
                    span: sp,
                }],
                span: sp,
            },
            span: sp,
            borrow_sig: None,
        }],
        evals: vec![],
    };
    let rt = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply("generate", json!("chunk"))))
        .default_model("mock-1")
        .build();
    let stream = run_agent(&ir, "relay", vec![Value::String(Arc::from("ctx"))], &rt)
        .await
        .expect("run");
    let err = collect_stream(stream).await.unwrap_err();
    assert!(matches!(
        err.kind,
        InterpErrorKind::BudgetExceeded { budget, used }
            if (budget - 0.50).abs() < 0.001 && (used - 0.75).abs() < 0.001
    ));
}

#[tokio::test]
async fn try_retry_retries_stream_when_first_element_is_err() {
    let src = "\
tool next_mode() -> Int

agent flaky_stream() -> Stream<Result<String, String>>:
    mode = next_mode()
    if mode == 0:
        yield Err(\"network\")
    yield Ok(\"done\")

agent caller() -> Stream<Result<String, String>>:
    for item in try flaky_stream() on error retry 3 times backoff exponential 1:
        yield item
";
    let ir = ir_of(src);
    let modes = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([0_i64, 1])));
    let rt = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .tool("next_mode", {
            let modes = Arc::clone(&modes);
            move |_| {
                let modes = Arc::clone(&modes);
                async move {
                    let next = modes.lock().unwrap().pop_front().unwrap_or(1);
                    Ok(json!(next))
                }
            }
        })
        .build();
    let stream = run_agent(&ir, "caller", vec![], &rt).await.expect("run");
    let items = collect_stream(stream).await.expect("collect");
    assert_eq!(
        items,
        vec![Value::ResultOk(crate::value::BoxedValue::new(Value::String(Arc::from("done"))))]
    );
}

#[tokio::test]
async fn try_retry_does_not_retry_mid_stream_err() {
    let src = "\
agent flaky_stream() -> Stream<Result<String, String>>:
    yield Ok(\"first\")
    yield Err(\"boom\")

agent caller() -> Stream<Result<String, String>>:
    for item in try flaky_stream() on error retry 3 times backoff exponential 1:
        yield item
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let stream = run_agent(&ir, "caller", vec![], &rt).await.expect("run");
    let items = collect_stream(stream).await.expect("collect");
    assert_eq!(
        items,
        vec![
            Value::ResultOk(crate::value::BoxedValue::new(Value::String(Arc::from("first")))),
            Value::ResultErr(crate::value::BoxedValue::new(Value::String(Arc::from("boom")))),
        ]
    );
}

#[tokio::test]
async fn bounded_stream_channel_round_trips_values() {
    let (sender, stream) = crate::value::StreamValue::channel(BackpressurePolicy::Bounded(2));
    assert!(sender.send(Ok(Value::Int(1))).await);
    assert!(sender.send(Ok(Value::Int(2))).await);
    drop(sender);
    let mut items = Vec::new();
    while let Some(item) = stream.next().await {
        items.push(item.expect("item"));
    }
    assert_eq!(items, vec![Value::Int(1), Value::Int(2)]);
}

#[tokio::test]
async fn unbounded_stream_channel_round_trips_values() {
    let (sender, stream) = crate::value::StreamValue::channel(BackpressurePolicy::Unbounded);
    assert!(sender.send(Ok(Value::String(Arc::from("a")))).await);
    drop(sender);
    let items = collect_stream(Value::Stream(stream)).await.expect("collect");
    assert_eq!(items, vec![Value::String(Arc::from("a"))]);
}

fn struct_string_field(value: &Value, field: &str) -> String {
    let Value::Struct(value) = value else {
        panic!("expected struct, got {value:?}");
    };
    match value.get_field(field).expect("field") {
        Value::String(value) => value.to_string(),
        other => panic!("expected string field, got {other:?}"),
    }
}
