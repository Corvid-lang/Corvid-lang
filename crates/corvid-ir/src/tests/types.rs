use super::*;

    #[test]
    fn lowers_grounded_type_refs_to_ir_grounded_types() {
        let src = "\
effect retrieval:
    data: grounded

tool grounded_echo(name: String) -> Grounded<String> uses retrieval

pub extern \"c\"
agent grounded_lookup(name: String) -> Grounded<String>:
    return grounded_echo(name)
";
        let ir = lower_src(src);
        assert!(matches!(
            &ir.tools[0].return_ty,
            corvid_types::Type::Grounded(inner) if matches!(&**inner, corvid_types::Type::String)
        ));
        assert!(matches!(
            &ir.agents[0].return_ty,
            corvid_types::Type::Grounded(inner) if matches!(&**inner, corvid_types::Type::String)
        ));
    }

    #[test]
    fn lowers_stream_partial_prompt_return_type() {
        let src = "\
type Plan:
    title: String
    body: String

prompt plan(topic: String) -> Stream<Partial<Plan>>:
    \"Plan {topic}\"
";
        let ir = lower_src(src);
        let prompt = &ir.prompts[0];
        match &prompt.return_ty {
            corvid_types::Type::Stream(inner) => match &**inner {
                corvid_types::Type::Partial(partial_inner) => {
                    assert!(matches!(&**partial_inner, corvid_types::Type::Struct(_)));
                }
                other => panic!("expected Partial<T>, got {other:?}"),
            },
            other => panic!("expected Stream<T>, got {other:?}"),
        }
    }
