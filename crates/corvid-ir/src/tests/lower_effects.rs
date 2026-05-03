use super::*;

    #[test]
    fn tool_effect_is_preserved_on_ir_tool() {
        let src = "\
tool send_email(to: String, body: String) -> Nothing dangerous

agent do_it(to: String) -> Nothing:
    approve SendEmail(to, to)
    return send_email(to, to)
";
        let ir = lower_src(src);
        assert_eq!(ir.tools.len(), 1);
        assert!(matches!(ir.tools[0].effect, Effect::Dangerous));
    }

    #[test]
    fn lowers_prompt_stream_metadata() {
        let src = "\
model expert:
    capability: expert

prompt generate(ctx: String) -> Stream<String>:
    with min_confidence 0.80
    with max_tokens 5000
    with backpressure bounded(100)
    with escalate_to expert
    \"Generate {ctx}\"
";
        let ir = lower_src(src);
        let prompt = &ir.prompts[0];
        assert_eq!(prompt.min_confidence, Some(0.80));
        assert_eq!(prompt.max_tokens, Some(5000));
        assert_eq!(prompt.backpressure, Some(BackpressurePolicy::Bounded(100)));
        assert_eq!(prompt.escalate_to.as_deref(), Some("expert"));
    }

    #[test]
    fn lowers_stream_resume_token_and_resume_call() {
        let src = "\
prompt draft(topic: String) -> Stream<String>:
    \"Draft {topic}\"

agent capture(topic: String) -> ResumeToken<String>:
    stream = draft(topic)
    return resume_token(stream)

agent continue_it(token: ResumeToken<String>) -> Stream<String>:
    return resume(draft, token)
";
        let ir = lower_src(src);
        let capture = ir
            .agents
            .iter()
            .find(|agent| agent.name == "capture")
            .expect("capture agent");
        match &capture.body.stmts[1] {
            IrStmt::Return {
                value: Some(value), ..
            } => {
                assert!(matches!(value.kind, IrExprKind::StreamResumeToken { .. }));
            }
            other => panic!("expected resume-token return, got {other:?}"),
        }

        let continue_it = ir
            .agents
            .iter()
            .find(|agent| agent.name == "continue_it")
            .expect("continue_it agent");
        match &continue_it.body.stmts[0] {
            IrStmt::Return {
                value: Some(value), ..
            } => match &value.kind {
                IrExprKind::ResumeStream { prompt_name, .. } => {
                    assert_eq!(prompt_name, "draft");
                }
                other => panic!("expected resume stream, got {other:?}"),
            },
            other => panic!("expected resume return, got {other:?}"),
        }
    }

    #[test]
    fn lowers_stream_split_merge_with_order_policy() {
        let src = "\
type Event:
    kind: String
    body: String

agent source() -> Stream<Event>:
    yield Event(\"b\", \"two\")
    yield Event(\"a\", \"one\")

agent fanout() -> Stream<Event>:
    return merge(source().split_by(\"kind\")).ordered_by(\"sorted\")
";
        let ir = lower_src(src);
        let fanout = ir
            .agents
            .iter()
            .find(|agent| agent.name == "fanout")
            .expect("fanout agent");
        match &fanout.body.stmts[0] {
            IrStmt::Return {
                value: Some(value), ..
            } => match &value.kind {
                IrExprKind::StreamMerge { groups, policy } => {
                    assert_eq!(*policy, StreamMergePolicy::Sorted);
                    assert!(matches!(groups.kind, IrExprKind::StreamSplitBy { .. }));
                }
                other => panic!("expected stream merge, got {other:?}"),
            },
            other => panic!("expected fanout return, got {other:?}"),
        }
    }

    #[test]
    fn lowers_calibrated_prompt_modifier() {
        let src = "\
prompt classify(ctx: String) -> String:
    calibrated
    \"Classify {ctx}.\"
";
        let ir = lower_src(src);
        assert!(ir.prompts[0].calibrated);
    }

    #[test]
    fn lowers_cacheable_prompt_modifier() {
        let src = "\
prompt classify(ctx: String) -> String:
    cacheable: true
    \"Classify {ctx}.\"
";
        let ir = lower_src(src);
        assert!(ir.prompts[0].cacheable);
    }

    #[test]
    fn lowers_prompt_cites_strictly_param_index() {
        let src = "\
prompt answer(question: String, ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    \"Answer from {ctx}\"
";
        let ir = lower_src(src);
        let prompt = &ir.prompts[0];
        assert_eq!(prompt.cites_strictly_param, Some(1));
    }

    #[test]
    fn grounded_unwrap_lowers_to_explicit_ir_node() {
        let src = "\
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent load(id: String) -> String:
    doc = fetch_doc(id)
    return doc.unwrap_discarding_sources()
";
        let ir = lower_src(src);
        let agent = ir.agents.iter().find(|a| a.name == "load").unwrap();
        let ret = agent
            .body
            .stmts
            .iter()
            .find_map(|stmt| match stmt {
                IrStmt::Return {
                    value: Some(value), ..
                } => Some(value),
                _ => None,
            })
            .expect("return value");
        assert!(
            matches!(ret.kind, IrExprKind::UnwrapGrounded { .. }),
            "expected UnwrapGrounded, got {:?}",
            ret.kind
        );
        assert_eq!(ret.ty, corvid_types::Type::String);
    }
