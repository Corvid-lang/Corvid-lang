use corvid_ast::Span;
use corvid_codegen_cl::{dup_drop::insert_dup_drop, pair_elim::eliminate_pairs};
use corvid_ir::{lower, IrAgent, IrBlock, IrExpr, IrExprKind, IrLiteral, IrParam, IrStmt};
use corvid_resolve::{resolve, DefId, LocalId};
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck, Type};

fn ir_of(src: &str) -> corvid_ir::IrFile {
    let tokens = lex(src).expect("lex");
    let (file, perr) = parse_file(&tokens);
    assert!(perr.is_empty(), "parse: {perr:?}");
    let resolved = resolve(&file);
    assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
    let checked = typecheck(&file, &resolved);
    assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
    lower(&file, &resolved, &checked)
}

fn count_dup_drop_agent(agent: &IrAgent) -> usize {
    count_dup_drop_block(&agent.body)
}

fn count_dup_drop_block(block: &IrBlock) -> usize {
    block.stmts.iter().map(count_dup_drop_stmt).sum()
}

fn count_dup_drop_stmt(stmt: &IrStmt) -> usize {
    let nested = match stmt {
        IrStmt::If {
            then_block,
            else_block,
            ..
        } => {
            count_dup_drop_block(then_block)
                + else_block
                    .as_ref()
                    .map(count_dup_drop_block)
                    .unwrap_or(0)
        }
        IrStmt::For { body, .. } => count_dup_drop_block(body),
        _ => 0,
    };
    nested
        + usize::from(matches!(stmt, IrStmt::Dup { .. } | IrStmt::Drop { .. }))
}

fn span() -> Span {
    Span { start: 0, end: 0 }
}

fn string_ty() -> Type {
    Type::String
}

fn local_expr(local_id: u32, ty: Type) -> IrExpr {
    IrExpr {
        kind: IrExprKind::Local {
            local_id: LocalId(local_id),
            name: format!("l{local_id}"),
        },
        ty,
        span: span(),
    }
}

#[test]
fn current_baseline_rc_count_fixtures_show_no_same_block_pair_reduction_yet() {
    let fixtures = [
        (
            "string_concat_chain",
            r#"
agent string_concat_chain() -> Int:
    s = "a" + "b" + "c" + "d" + "e"
    if s == "abcde":
        return 1
    return 0
"#,
        ),
        (
            "struct_build_and_destructure",
            r#"
type Pair:
    left: String
    right: String

agent struct_build_and_destructure() -> Int:
    p = Pair("hello", "world")
    l = p.left
    r = p.right
    if l == "hello":
        return 1
    return 0
"#,
        ),
        (
            "list_of_strings_iter",
            r#"
agent list_of_strings_iter() -> Int:
    xs = ["alpha", "beta", "gamma"]
    n = 0
    for s in xs:
        if s == "beta":
            n = n + 1
    return n
"#,
        ),
        (
            "local_arg_to_borrowed_callee",
            r#"
agent echo(s: String) -> String:
    return s

agent main() -> Int:
    x = "shared"
    a = echo(x)
    b = echo(x)
    if a == "shared":
        return 1
    return 0
"#,
        ),
        (
            "passthrough_agent",
            r#"
agent echo(s: String) -> String:
    return s

agent main() -> Int:
    a = echo("one")
    b = echo("two")
    if a == "one":
        return 1
    return 0
"#,
        ),
    ];

    for (name, src) in fixtures {
        let ir = ir_of(src);
        let before: usize = ir
            .agents
            .iter()
            .map(|agent| count_dup_drop_agent(&insert_dup_drop(agent)))
            .sum();
        let after: usize = ir
            .agents
            .iter()
            .map(|agent| count_dup_drop_agent(&eliminate_pairs(insert_dup_drop(agent))))
            .sum();
        eprintln!("PAIR_ELIM BASELINE {name}: before={before} after={after}");
        assert_eq!(
            after, before,
            "current baseline fixture `{name}` unexpectedly changed shape under same-block pair elimination"
        );
    }
}

#[test]
fn benchmark_shaped_fixture_has_nonzero_dup_drop_reduction() {
    let agent = IrAgent {
        id: DefId(0),
        name: "bench_fixture".into(),
        extern_abi: None,
        params: vec![IrParam {
            name: "s".into(),
            local_id: LocalId(0),
            ty: string_ty(),
            span: span(),
        }],
        return_ty: Type::Int,
        cost_budget: None,
        body: IrBlock {
            stmts: vec![
                IrStmt::Dup {
                    local_id: LocalId(0),
                    span: span(),
                },
                IrStmt::Let {
                    local_id: LocalId(1),
                    name: "t".into(),
                    ty: string_ty(),
                    value: local_expr(0, string_ty()),
                    span: span(),
                },
                IrStmt::Let {
                    local_id: LocalId(2),
                    name: "x".into(),
                    ty: Type::Int,
                    value: IrExpr {
                        kind: IrExprKind::Literal(IrLiteral::Int(1)),
                        ty: Type::Int,
                        span: span(),
                    },
                    span: span(),
                },
                IrStmt::Drop {
                    local_id: LocalId(0),
                    span: span(),
                },
                IrStmt::Return {
                    value: Some(IrExpr {
                        kind: IrExprKind::Literal(IrLiteral::Int(0)),
                        ty: Type::Int,
                        span: span(),
                    }),
                    span: span(),
                },
            ],
            span: span(),
        },
        span: span(),
        borrow_sig: None,
    };

    let before = count_dup_drop_agent(&agent);
    let reduced = eliminate_pairs(agent);
    let after = count_dup_drop_agent(&reduced);

    assert_eq!(before, 2, "fixture should start with one Dup and one Drop");
    assert_eq!(after, 0, "pair elimination should remove the redundant pair");
    assert!(
        matches!(reduced.body.stmts[0], IrStmt::Let { local_id: LocalId(1), .. }),
        "use statement should remain after eliminating the pair"
    );
}
