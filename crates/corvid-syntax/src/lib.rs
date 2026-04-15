//! Lexer and parser for Corvid.
//!
//! Produces AST from source text. See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

pub mod errors;
pub mod lexer;
pub mod parser;
pub mod token;

pub use errors::{LexError, LexErrorKind, ParseError, ParseErrorKind};
pub use lexer::lex;
pub use parser::{parse_block, parse_expr, parse_file};
pub use token::{TokKind, Token};

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip spans; keep just `TokKind` sequence — makes tests readable.
    fn kinds(source: &str) -> Vec<TokKind> {
        lex(source).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_empty_file() {
        let toks = kinds("");
        assert_eq!(toks, vec![TokKind::Eof]);
    }

    #[test]
    fn lexes_single_keyword() {
        let toks = kinds("agent");
        assert_eq!(
            toks,
            vec![TokKind::KwAgent, TokKind::Newline, TokKind::Eof]
        );
    }

    #[test]
    fn distinguishes_keywords_from_idents() {
        let toks = kinds("agent myagent");
        assert_eq!(
            toks,
            vec![
                TokKind::KwAgent,
                TokKind::Ident("myagent".into()),
                TokKind::Newline,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_all_keywords() {
        // Smoke test — verify every v0.1 keyword parses as its own TokKind.
        let src = "agent tool prompt type import as \
                   extend public package \
                   try on error retry times backoff linear exponential \
                   approve dangerous \
                   if else for in return break continue pass \
                   true false nothing \
                   and or not";
        let toks = kinds(src);
        let expected = vec![
            TokKind::KwAgent,
            TokKind::KwTool,
            TokKind::KwPrompt,
            TokKind::KwType,
            TokKind::KwImport,
            TokKind::KwAs,
            TokKind::KwExtend,
            TokKind::KwPublic,
            TokKind::KwPackage,
            TokKind::KwTry,
            TokKind::KwOn,
            TokKind::KwError,
            TokKind::KwRetry,
            TokKind::KwTimes,
            TokKind::KwBackoff,
            TokKind::KwLinear,
            TokKind::KwExponential,
            TokKind::KwApprove,
            TokKind::KwDangerous,
            TokKind::KwIf,
            TokKind::KwElse,
            TokKind::KwFor,
            TokKind::KwIn,
            TokKind::KwReturn,
            TokKind::KwBreak,
            TokKind::KwContinue,
            TokKind::KwPass,
            TokKind::KwTrue,
            TokKind::KwFalse,
            TokKind::KwNothing,
            TokKind::KwAnd,
            TokKind::KwOr,
            TokKind::KwNot,
            TokKind::Newline,
            TokKind::Eof,
        ];
        assert_eq!(toks, expected);
    }

    #[test]
    fn dropped_keywords_are_idents() {
        // Words dropped in the simplification (`let`, `function`, `from`,
        // `effect`, `pure`, `compensable`, `irreversible`) should now lex
        // as plain identifiers.
        for word in ["let", "function", "from", "effect", "pure", "compensable", "irreversible"] {
            let toks = kinds(word);
            assert!(
                matches!(toks[0], TokKind::Ident(ref s) if s == word),
                "{word} should be an Ident, got {:?}",
                toks[0]
            );
        }
    }

    #[test]
    fn lexes_integer_and_float() {
        let toks = kinds("42 3.14");
        assert_eq!(
            toks,
            vec![
                TokKind::Int(42),
                TokKind::Float(3.14),
                TokKind::Newline,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn dot_after_integer_is_field_access() {
        // `x.y` should lex as Ident Dot Ident.
        // `42.foo` should lex as Int Dot Ident (not a malformed float).
        let toks = kinds("ticket.order_id");
        assert_eq!(
            toks,
            vec![
                TokKind::Ident("ticket".into()),
                TokKind::Dot,
                TokKind::Ident("order_id".into()),
                TokKind::Newline,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_simple_string() {
        let toks = kinds(r#""hello""#);
        assert_eq!(
            toks,
            vec![
                TokKind::StringLit("hello".into()),
                TokKind::Newline,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_string_escapes() {
        let toks = kinds(r#""a\n\t\"b""#);
        assert_eq!(
            toks,
            vec![
                TokKind::StringLit("a\n\t\"b".into()),
                TokKind::Newline,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn rejects_unterminated_string() {
        let err = lex(r#""hello"#).unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(matches!(err[0].kind, LexErrorKind::UnterminatedString));
    }

    #[test]
    fn rejects_newline_in_single_string() {
        let err = lex("\"a\nb\"").unwrap_err();
        assert!(err.iter().any(|e| matches!(e.kind, LexErrorKind::UnterminatedString)));
    }

    #[test]
    fn lexes_triple_quoted_string() {
        let toks = kinds("\"\"\"line 1\nline 2\"\"\"");
        assert_eq!(
            toks,
            vec![
                TokKind::StringLit("line 1\nline 2".into()),
                TokKind::Newline,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_all_operators() {
        let src = "( ) [ ] : , . -> ? = == != < <= > >= + - * / %";
        let toks = kinds(src);
        let expected = vec![
            TokKind::LParen,
            TokKind::RParen,
            TokKind::LBracket,
            TokKind::RBracket,
            TokKind::Colon,
            TokKind::Comma,
            TokKind::Dot,
            TokKind::Arrow,
            TokKind::Question,
            TokKind::Assign,
            TokKind::Eq,
            TokKind::NotEq,
            TokKind::Lt,
            TokKind::LtEq,
            TokKind::Gt,
            TokKind::GtEq,
            TokKind::Plus,
            TokKind::Minus,
            TokKind::Star,
            TokKind::Slash,
            TokKind::Percent,
            TokKind::Newline,
            TokKind::Eof,
        ];
        assert_eq!(toks, expected);
    }

    #[test]
    fn comments_are_skipped() {
        let toks = kinds("# this is a comment\nagent");
        assert_eq!(
            toks,
            vec![TokKind::KwAgent, TokKind::Newline, TokKind::Eof]
        );
    }

    #[test]
    fn blank_lines_do_not_affect_indent() {
        let src = "agent a:\n\n    pass\n";
        let toks = kinds(src);
        assert_eq!(
            toks,
            vec![
                TokKind::KwAgent,
                TokKind::Ident("a".into()),
                TokKind::Colon,
                TokKind::Newline,
                TokKind::Indent,
                TokKind::KwPass,
                TokKind::Newline,
                TokKind::Dedent,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn indent_and_dedent_nest() {
        let src = "\
agent a:
    x = 1
    if x:
        y = 2
    return x
";
        let toks = kinds(src);
        let expected = vec![
            TokKind::KwAgent,
            TokKind::Ident("a".into()),
            TokKind::Colon,
            TokKind::Newline,
            TokKind::Indent,
            TokKind::Ident("x".into()),
            TokKind::Assign,
            TokKind::Int(1),
            TokKind::Newline,
            TokKind::KwIf,
            TokKind::Ident("x".into()),
            TokKind::Colon,
            TokKind::Newline,
            TokKind::Indent,
            TokKind::Ident("y".into()),
            TokKind::Assign,
            TokKind::Int(2),
            TokKind::Newline,
            TokKind::Dedent,
            TokKind::KwReturn,
            TokKind::Ident("x".into()),
            TokKind::Newline,
            TokKind::Dedent,
            TokKind::Eof,
        ];
        assert_eq!(toks, expected);
    }

    #[test]
    fn multiple_dedents_to_eof() {
        // Deep nesting that ends at EOF should emit all the Dedents.
        let src = "\
a:
    b:
        c
";
        let toks = kinds(src);
        let dedents = toks
            .iter()
            .filter(|t| matches!(t, TokKind::Dedent))
            .count();
        assert_eq!(dedents, 2);
    }

    #[test]
    fn tabs_in_indent_are_rejected() {
        let src = "agent a:\n\tpass\n";
        let err = lex(src).unwrap_err();
        assert!(err.iter().any(|e| matches!(e.kind, LexErrorKind::TabIndentation)));
    }

    #[test]
    fn newlines_inside_brackets_are_ignored() {
        // Multi-line call should not produce Newline/Indent/Dedent tokens
        // from the nested lines.
        let src = "f(\n    a,\n    b,\n)";
        let toks = kinds(src);
        assert_eq!(
            toks,
            vec![
                TokKind::Ident("f".into()),
                TokKind::LParen,
                TokKind::Ident("a".into()),
                TokKind::Comma,
                TokKind::Ident("b".into()),
                TokKind::Comma,
                TokKind::RParen,
                TokKind::Newline,
                TokKind::Eof,
            ]
        );
    }

    #[test]
    fn rejects_stray_bang() {
        let err = lex("a ! b").unwrap_err();
        assert!(err
            .iter()
            .any(|e| matches!(e.kind, LexErrorKind::UnexpectedChar('!'))));
    }

    #[test]
    fn lexes_full_refund_bot_example() {
        // Canonical Corvid program. Must lex without error.
        let src = r#"
tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
"#;
        let result = lex(src);
        assert!(result.is_ok(), "lex errors: {:?}", result.err());
        let toks = result.unwrap();
        // Spot-check: should contain all the important keywords.
        let has = |want: &TokKind| toks.iter().any(|t| &t.kind == want);
        assert!(has(&TokKind::KwTool));
        assert!(has(&TokKind::KwAgent));
        assert!(has(&TokKind::KwApprove));
        assert!(has(&TokKind::KwDangerous));
        assert!(has(&TokKind::KwReturn));
    }

    #[test]
    fn spans_point_at_source() {
        let toks = lex("agent x").unwrap();
        assert_eq!(toks[0].kind, TokKind::KwAgent);
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 5);
        assert_eq!(toks[1].kind, TokKind::Ident("x".into()));
        assert_eq!(toks[1].span.start, 6);
        assert_eq!(toks[1].span.end, 7);
    }
}
