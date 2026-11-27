//! # The parser (`parser.rs`)
//!
//! **What this file is:** the second compiler stage ("syntax analysis"). It
//! reads the token list from the lexer and builds the syntax tree
//! (`ast::Program`), checking that tokens come in a grammatical order.
//!
//! **Pipeline position:** `&[Token]` in → `(ast::Program, Vec<Diagnostic>)`
//! out → `resolve.rs` / `codegen.rs`. Entry point: `parse`.
//!
//! **Textbook chapter:** `docs/04-syntax-analysis.md`.
//!
//! ## Technique
//!
//! - **Recursive descent** for statements and blocks: one method per grammar
//!   rule (`program`, `item`, `end_of_line`).
//! - **Pratt parsing** (top-down operator precedence) for expressions:
//!   `expr(min_bp)` with a *binding power* per operator (`Parser::expr`).
//! - **Error recovery**: on an error the parser records a diagnostic and
//!   skips to the end of the logical line (`sync_line`), so one run reports
//!   several errors.
//!
//! ## Precedence (syntax.md S11, Python order; M1 subset in brackets)
//!
//! `|>` < `or` < `and` < `not` < comparisons < [`+ -`] < [`* / // %`]
//! < [unary `-`] < [`**` right-assoc] < postfix. As in Python, `-2 ** 2` is
//! `-(2 ** 2)` and the right operand of `**` may be negated: `2 ** -1`.
//!
//! | operator           | left bp | right bp | associativity |
//! |--------------------|---------|----------|---------------|
//! | `+` `-`            | 10      | 11       | left          |
//! | `*` `/` `//` `%`   | 20      | 21       | left          |
//! | prefix `-`         | —       | 30       | prefix        |
//! | `**`               | 41      | 40       | right         |
//!
//! ## Where to change things
//!
//! | I want to…                                  | Look at                                               |
//! |---------------------------------------------|-------------------------------------------------------|
//! | add a binary operator / change precedence   | `Parser::expr`, the `match t.kind` table of `(op, l_bp, r_bp)` |
//! | add a prefix form (`not x`, literals, names)| `Parser::prefix`                                      |
//! | add postfix forms (calls `f(x)`, `.field`)  | `Parser::expr` loop (currently reports E0009 for `(` `[` `.` `?`) |
//! | add statements (`let`, `fn`, `def`)         | `Parser::item` (dispatch on the first token)          |
//! | add indented blocks                         | handle `Indent` … `Dedent` tokens; see `skip_block` for the shape |
//! | change error recovery                       | `sync_line`, `skip_block`                             |
//!
//! Syntax error codes: E0101 unexpected token, E0103 unclosed `(`,
//! E0009 construct not supported yet.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `type PResult<T> = Result<T, Failed>` — a type alias; `Failed` is a
//!   marker meaning "an error was already recorded" (*Option and Result*).
//! - `?` to bail out of nested parsing functions on the first error (*Option and Result*).
//! - `Box::new(expr)` to put child nodes on the heap (*Ownership and borrowing*).
//! - nested or-patterns: `TokenKind::Kw(Keyword::And | Keyword::Or)` (*Pattern matching*).
//! - `.clone()` of a token to keep it while advancing (*Ownership and borrowing*).

use crate::ast::{BinOp, Expr, ExprKind, Item, NodeId, Program, UnaryOp};
use crate::diagnostic::Diagnostic;
use crate::token::{Keyword, Token, TokenKind};

/// Binding power of prefix `-`: binds tighter than `*`, looser than `**`.
const PREFIX_NEG_BP: u8 = 30;

/// Parse a token stream (which must end with `EOF`).
pub fn parse(tokens: &[Token]) -> (Program, Vec<Diagnostic>) {
    let mut p = Parser {
        tokens,
        pos: 0,
        diags: Vec::new(),
        next_id: 0,
    };
    let program = p.program();
    (program, p.diags)
}

/// Marker: a diagnostic was recorded; the caller should recover.
// RUST: a struct with no fields carries no data; it only says "failed".
struct Failed;

/// Result of a parsing function.
type PResult<T> = Result<T, Failed>;

/// Parser state: a cursor over the token slice.
struct Parser<'t> {
    tokens: &'t [Token],
    /// Index of the current token.
    pos: usize,
    diags: Vec<Diagnostic>,
    /// Next `NodeId` to hand out.
    next_id: u32,
}

impl Parser<'_> {
    // ---- cursor helpers ------------------------------------------------

    /// The current token (EOF forever once the end is reached).
    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    /// Consume and return the current token.
    fn bump(&mut self) -> Token {
        let t = self.peek().clone();
        if t.kind != TokenKind::Eof {
            self.pos += 1;
        }
        t
    }

    /// A fresh node id.
    fn id(&mut self) -> NodeId {
        self.next_id += 1;
        NodeId(self.next_id - 1)
    }

    /// Record a diagnostic and return the `Failed` marker, so callers can
    /// write `return Err(self.error(...))`.
    fn error(&mut self, d: Diagnostic) -> Failed {
        self.diags.push(d);
        Failed
    }

    // ---- statements ----------------------------------------------------

    /// program := (NEWLINE | item)* EOF
    fn program(&mut self) -> Program {
        let mut items = Vec::new();
        loop {
            match self.peek().kind {
                TokenKind::Eof => break,
                TokenKind::Newline | TokenKind::Dedent => {
                    self.bump();
                }
                TokenKind::Indent => {
                    // No construct opens a block yet, so any indentation at
                    // top level is an error.
                    let span = self.peek().span;
                    self.diags.push(
                        Diagnostic::new("E0101", "unexpected indentation", span)
                            .label("this line is indented, but no block was opened"),
                    );
                    self.skip_block();
                }
                _ => match self.item() {
                    Ok(item) => items.push(item),
                    Err(Failed) => self.sync_line(),
                },
            }
        }
        Program { items }
    }

    /// item := expr (NEWLINE | EOF)
    ///
    /// EXTEND (M2+): look at the first token here to dispatch to `let`,
    /// `fn`, `def`, `assert`, … parsers.
    fn item(&mut self) -> PResult<Item> {
        let e = self.expr(0)?;
        self.end_of_line()?;
        Ok(Item::Expr(e))
    }

    /// Expect the end of a logical line.
    fn end_of_line(&mut self) -> PResult<()> {
        match self.peek().kind {
            TokenKind::Newline => {
                self.bump();
                Ok(())
            }
            TokenKind::Eof => Ok(()),
            _ => {
                let t = self.peek().clone();
                Err(self.error(
                    Diagnostic::new(
                        "E0101",
                        format!("expected end of line, found {}", t.kind.describe()),
                        t.span,
                    )
                    .label("unexpected here"),
                ))
            }
        }
    }

    // ---- error recovery ------------------------------------------------

    /// Skip to just after the next NEWLINE (or to EOF).
    fn sync_line(&mut self) {
        loop {
            match self.bump().kind {
                TokenKind::Newline | TokenKind::Eof => break,
                TokenKind::Indent => {
                    self.skip_block();
                    break;
                }
                _ => {}
            }
        }
    }

    /// Skip an INDENT ... matching DEDENT region (the INDENT is current).
    fn skip_block(&mut self) {
        let mut depth = 0u32;
        loop {
            match self.bump().kind {
                TokenKind::Indent => depth += 1,
                TokenKind::Dedent => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => break,
                _ => {}
            }
        }
    }

    // ---- expressions (Pratt) -------------------------------------------

    /// Parse an expression whose operators all bind at least as tightly as
    /// `min_bp`.
    ///
    /// The Pratt loop: parse a prefix (literal, `-x`, `( ... )`), then, while
    /// the next token is an infix operator with left binding power
    /// `l_bp >= min_bp`, consume it and parse its right side with `r_bp`.
    /// `r_bp > l_bp` makes an operator left-associative
    /// (`10 - 4 - 3 = (10 - 4) - 3`); `r_bp < l_bp` makes it right-associative
    /// (`2 ** 3 ** 2 = 2 ** (3 ** 2)`).
    fn expr(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.prefix()?;
        loop {
            let t = self.peek().clone();
            // EXTEND: new infix operators get a row here.
            let (op, l_bp, r_bp) = match t.kind {
                TokenKind::Plus => (BinOp::Add, 10, 11),
                TokenKind::Minus => (BinOp::Sub, 10, 11),
                TokenKind::Star => (BinOp::Mul, 20, 21),
                TokenKind::Slash => (BinOp::Div, 20, 21),
                TokenKind::SlashSlash => (BinOp::IDiv, 20, 21),
                TokenKind::Percent => (BinOp::Mod, 20, 21),
                // Right-associative; the right side starts over at prefix level,
                // which admits `2 ** -1`.
                TokenKind::StarStar => (BinOp::Pow, 41, 40),
                // Known operators of later milestones: say so clearly
                // instead of a confusing "expected end of line".
                TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
                | TokenKind::PipeGt
                | TokenKind::Kw(Keyword::And | Keyword::Or | Keyword::In)
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::Dot
                | TokenKind::Question => {
                    return Err(self.error(
                        Diagnostic::new(
                            "E0009",
                            format!("{} is not supported yet", t.kind.describe()),
                            t.span,
                        )
                        .label("not implemented in this version of valo"),
                    ));
                }
                // Anything else ends the expression (NEWLINE, `)`, ...).
                _ => break,
            };
            if l_bp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.expr(r_bp)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr {
                id: self.id(),
                span,
                kind: ExprKind::Binary {
                    op,
                    op_span: t.span,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            };
        }
        Ok(lhs)
    }

    /// Prefix position: literals, unary operators, parenthesised groups.
    /// EXTEND: names (M2), `true/false` (M3), `if` (M3), lambdas (M6),
    /// strings (M7) and list literals (M8) start here.
    fn prefix(&mut self) -> PResult<Expr> {
        let t = self.bump();
        match t.kind {
            TokenKind::Int(n) => Ok(Expr {
                id: self.id(),
                span: t.span,
                kind: ExprKind::Int(n),
            }),
            TokenKind::Minus => {
                let operand = self.expr(PREFIX_NEG_BP)?;
                let span = t.span.to(operand.span);
                Ok(Expr {
                    id: self.id(),
                    span,
                    kind: ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                })
            }
            TokenKind::LParen => {
                // Grouping: parse the inside at the lowest binding power.
                // No AST node is created for the parentheses themselves; the
                // tree shape already records the grouping.
                let mut inner = self.expr(0)?;
                let close = self.peek().clone();
                if close.kind == TokenKind::RParen {
                    self.bump();
                    inner.span = t.span.to(close.span);
                    return Ok(inner);
                }
                let d = if matches!(close.kind, TokenKind::Newline | TokenKind::Eof) {
                    Diagnostic::new("E0103", "unclosed `(`", t.span)
                        .label("opened here, never closed")
                } else {
                    Diagnostic::new(
                        "E0101",
                        format!("expected `)`, found {}", close.kind.describe()),
                        close.span,
                    )
                    .label("expected `)` here")
                    .with(t.span, "to close this `(`")
                };
                Err(self.error(d))
            }
            // Valid syntax of later milestones: report "not supported yet".
            TokenKind::Float(_)
            | TokenKind::Ident(_)
            | TokenKind::Kw(
                Keyword::Let
                | Keyword::Fn
                | Keyword::Def
                | Keyword::If
                | Keyword::Match
                | Keyword::Not
                | Keyword::True
                | Keyword::False
                | Keyword::None
                | Keyword::Import
                | Keyword::From
                | Keyword::Assert
                | Keyword::Test
                | Keyword::Mock,
            )
            | TokenKind::LBracket
            | TokenKind::LBrace => {
                let what = match &t.kind {
                    TokenKind::Float(_) => "float literals are".to_string(),
                    TokenKind::Ident(_) => "names are".to_string(),
                    TokenKind::LBracket => "list literals are".to_string(),
                    TokenKind::LBrace => "dict literals are".to_string(),
                    other => format!("`{other}` is"),
                };
                Err(self.error(
                    Diagnostic::new("E0009", format!("{what} not supported yet"), t.span)
                        .label("not implemented in this version of valo"),
                ))
            }
            other => {
                // Do not swallow line structure while reporting: step back so
                // `sync_line` sees the NEWLINE/INDENT/DEDENT we just consumed.
                if matches!(
                    other,
                    TokenKind::Newline | TokenKind::Eof | TokenKind::Dedent | TokenKind::Indent
                ) {
                    self.pos -= usize::from(other != TokenKind::Eof);
                }
                Err(self.error(
                    Diagnostic::new(
                        "E0101",
                        format!("expected an expression, found {}", other.describe()),
                        t.span,
                    )
                    .label("expected an expression here"),
                ))
            }
        }
    }
}

// Tests: `cargo test -p valo-compiler parser`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ast, lexer};

    /// Parse and return the S-expression dump; panics on any error.
    fn parse_ok(src: &str) -> String {
        let l = lexer::lex(src);
        assert!(l.diags.is_empty(), "{:?}", l.diags);
        let (p, d) = parse(&l.tokens);
        assert!(d.is_empty(), "{d:?}");
        ast::dump(&p)
    }

    /// Error codes for a source.
    fn parse_codes(src: &str) -> Vec<&'static str> {
        let l = lexer::lex(src);
        parse(&l.tokens).1.iter().map(|d| d.code).collect()
    }

    // Snapshot: src/snapshots/valo_compiler__parser__tests__precedence_and_associativity.snap
    #[test]
    fn precedence_and_associativity() {
        insta::assert_snapshot!(parse_ok(
            "1 + 2 * 3\n(1 + 2) * 3\n10 - 4 - 3\n2 ** 3 ** 2\n-2 ** 2\n2 ** -1\n-7 // 2 % 3\n- - 1\n2 ** -1 * 3"
        ));
    }

    #[test]
    fn errors_recover_per_line() {
        assert_eq!(parse_codes("1 +\n2 2\n3\n(1"), ["E0101", "E0101", "E0103"]);
        assert_eq!(parse_codes("  1"), ["E0101"]);
        assert_eq!(
            parse_codes("x + 1\n1 == 2\n1.5"),
            ["E0009", "E0009", "E0009"]
        );
        assert_eq!(parse_codes("(1 2)"), ["E0101"]);
    }
}
