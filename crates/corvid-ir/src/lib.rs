//! Intermediate representation for the Corvid compiler.
//!
//! Post-typecheck, pre-codegen. Desugared, normalized, and carrying
//! resolved references plus attached types.
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

mod imports;
pub mod lower;
pub mod types;

pub use lower::{lower, lower_with_modules};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::{Backoff, BackpressurePolicy, Effect};
    use corvid_resolve::resolve;
    use corvid_syntax::{lex, parse_file};
    use corvid_types::typecheck;

    fn lower_src(src: &str) -> IrFile {
        let tokens = lex(src).expect("lex");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse: {perr:?}");
        let resolved = resolve(&file);
        assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
        let checked = typecheck(&file, &resolved);
        assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
        lower(&file, &resolved, &checked)
    }

    #[path = "lower_basic.rs"]
    mod lower_basic;

    #[path = "lower_effects.rs"]
    mod lower_effects;

    #[path = "lower_replay.rs"]
    mod lower_replay;

    #[path = "lower_imports.rs"]
    mod lower_imports;




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







}
