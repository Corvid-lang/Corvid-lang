//! Statement + block parsing.
//!
//! Covers `parse_indented_block` (the `Indent … Dedent` block
//! shape used by every body — agent/prompt/eval/if/for) plus each
//! individual statement parser: return, yield, if, for, approve,
//! break/continue/pass, assign-or-expr, and bare-expr statements.
//!
//! Extracted from `parser.rs` as part of Phase 20i responsibility
//! decomposition. All methods operate on `Parser<'a>` state via
//! an additional `impl<'a> Parser<'a>` block.

use super::Parser;
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::{Block, Expr, Ident, Span, Stmt};

impl<'a> Parser<'a> {
    // ------------------------------------------------------------
    // Block parsing.
    // ------------------------------------------------------------

    /// Expect `Indent`, then 1+ statements, then `Dedent`.
    pub(super) fn parse_indented_block(&mut self) -> Result<Block, ParseError> {
        let start_span = self.peek_span();
        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: start_span,
            });
        }
        self.bump(); // consume Indent

        let mut stmts = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(e) => {
                    self.errors.push(e);
                    self.sync_to_statement_boundary();
                }
            }
        }
        let end_span = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        if stmts.is_empty() {
            self.errors.push(ParseError {
                kind: ParseErrorKind::EmptyBlock,
                span: start_span,
            });
        }

        Ok(Block {
            stmts,
            span: start_span.merge(end_span),
        })
    }

    // ------------------------------------------------------------
    // Statement parsing.
    // ------------------------------------------------------------

    pub(super) fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        // Skip stray newlines (blank lines inside blocks).
        while matches!(self.peek(), TokKind::Newline) {
            self.bump();
        }

        match self.peek() {
            TokKind::KwReturn => self.parse_return_stmt(),
            TokKind::KwYield => self.parse_yield_stmt(),
            TokKind::KwIf => self.parse_if_stmt(),
            TokKind::KwFor => self.parse_for_stmt(),
            TokKind::KwApprove => self.parse_approve_stmt(),
            TokKind::KwBreak => self.parse_simple_kw_stmt(TokKind::KwBreak, |_| {
                // We don't have a dedicated Break variant yet — represent it as
                // an expression statement referencing the keyword would be wrong.
                // Use a placeholder: treat Break/Continue/Pass as expression-less
                // marker statements via Stmt::Expr with a specific Ident.
                // For cleanness we'll add variants when the AST needs them.
                unreachable!("handled specially")
            }),
            TokKind::KwContinue => self.parse_simple_kw_stmt(TokKind::KwContinue, |_| {
                unreachable!("handled specially")
            }),
            TokKind::KwPass => self.parse_simple_kw_stmt(TokKind::KwPass, |_| {
                unreachable!("handled specially")
            }),
            TokKind::Ident(_) => self.parse_assign_or_expr_stmt(),
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // return
        let value = if matches!(self.peek(), TokKind::Newline | TokKind::Eof) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let end = self.peek_span();
        self.expect_newline()?;
        Ok(Stmt::Return {
            value,
            span: start.merge(end),
        })
    }

    fn parse_yield_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // yield
        let value = self.parse_expr()?;
        let end = value.span();
        self.expect_newline()?;
        Ok(Stmt::Yield {
            value,
            span: start.merge(end),
        })
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // if
        let cond = self.parse_expr()?;
        self.expect(TokKind::Colon, "`:` after `if` condition")?;
        self.expect_newline()?;
        let then_block = self.parse_indented_block()?;
        let else_block = if matches!(self.peek(), TokKind::KwElse) {
            self.bump();
            self.expect(TokKind::Colon, "`:` after `else`")?;
            self.expect_newline()?;
            Some(self.parse_indented_block()?)
        } else {
            None
        };
        let end = else_block
            .as_ref()
            .map(|b| b.span)
            .unwrap_or(then_block.span);
        Ok(Stmt::If {
            cond,
            then_block,
            else_block,
            span: start.merge(end),
        })
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // for
        let (var_name, var_span) = self.expect_ident()?;
        self.expect(TokKind::KwIn, "`in` in `for` loop")?;
        let iter = self.parse_expr()?;
        self.expect(TokKind::Colon, "`:` after `for` clause")?;
        self.expect_newline()?;
        let body = self.parse_indented_block()?;
        let end = body.span;
        Ok(Stmt::For {
            var: Ident::new(var_name, var_span),
            iter,
            body,
            span: start.merge(end),
        })
    }

    fn parse_approve_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // approve
        let action = self.parse_expr()?;
        let end = action.span();
        self.expect_newline()?;
        Ok(Stmt::Approve {
            action,
            span: start.merge(end),
        })
    }

    /// `break`, `continue`, and `pass` are each a single keyword + newline.
    /// Since the AST doesn't yet have dedicated variants, they are encoded
    /// as expression statements containing a sentinel `Ident` — the name
    /// resolver will recognize them. (A future cleanup: add real variants.)
    fn parse_simple_kw_stmt(
        &mut self,
        _expected: TokKind,
        _: fn(Span) -> Stmt,
    ) -> Result<Stmt, ParseError> {
        let span = self.peek_span();
        let kw = self.peek().clone();
        self.bump();
        self.expect_newline()?;
        let name = match kw {
            TokKind::KwBreak => "break",
            TokKind::KwContinue => "continue",
            TokKind::KwPass => "pass",
            _ => unreachable!(),
        };
        Ok(Stmt::Expr {
            expr: Expr::Ident {
                name: Ident::new(name, span),
                span,
            },
            span,
        })
    }

    /// `IDENT '=' expr NEWLINE` is an assignment; anything else is an
    /// expression statement.
    fn parse_assign_or_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        // Peek two ahead: IDENT then `=` ? → assignment.
        if matches!(self.peek(), TokKind::Ident(_))
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokKind::Assign)
            )
        {
            let start = self.peek_span();
            let (name, name_span) = self.expect_ident()?;
            self.bump(); // =
            let value = self.parse_expr()?;
            let end = value.span();
            self.expect_newline()?;
            return Ok(Stmt::Let {
                name: Ident::new(name, name_span),
                ty: None,
                value,
                span: start.merge(end),
            });
        }
        self.parse_expr_stmt()
    }

    fn parse_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        let expr = self.parse_expr()?;
        let span = expr.span();
        self.expect_newline()?;
        Ok(Stmt::Expr { expr, span })
    }
}
