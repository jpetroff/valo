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
//! - **Recursive descent** for statements, blocks, `if` and type annotations:
//!   one method per grammar rule (`program`, `item`, `let_binding`,
//!   `if_expr`, `branch`, `block`, `type_expr`…).
//! - **Pratt parsing** (top-down operator precedence) for expressions:
//!   `expr(min_bp)` with a *binding power* per operator (`Parser::expr`).
//! - **Error recovery**: on an error the parser records a diagnostic and
//!   skips to the end of the logical line (`sync_line`), so one run reports
//!   several errors.
//!
//! ## Precedence (syntax.md S11, Python order)
//!
//! `|>` (M4) < `or` < `and` < `not` < comparisons < `+ -` < `* / // %`
//! < unary `-` < `**` (right-assoc) < postfix (M4+). As in Python, `-2 ** 2`
//! is `-(2 ** 2)`, the right operand of `**` may be negated (`2 ** -1`), and
//! `not a == b` is `not (a == b)`.
//!
//! | operator                   | left bp | right bp | associativity |
//! |----------------------------|---------|----------|---------------|
//! | `or`                       | 3       | 4        | left          |
//! | `and`                      | 5       | 6        | left          |
//! | prefix `not`               | —       | 7        | prefix        |
//! | `== != < <= > >=`          | 8       | 9        | chained       |
//! | `+` `-`                    | 10      | 11       | left          |
//! | `*` `/` `//` `%`           | 20      | 21       | left          |
//! | prefix `-`                 | —       | 30       | prefix        |
//! | `**`                       | 41      | 40       | right         |
//!
//! Comparisons *chain* (Python): `a < b <= c` becomes one `Compare` node
//! with two links, not `(a < b) <= c` (`Parser::compare_chain`).
//!
//! ## Layout: lines, blocks and `if` (syntax.md S6)
//!
//! - A block is `NEWLINE INDENT item+ DEDENT` after a `:` (`Parser::branch`);
//!   its last item must be an expression (E0104).
//! - The lexer emits `NEWLINE` *before* `DEDENT`, so an expression that ends
//!   with a block has already consumed its line: `end_of_line` accepts a
//!   just-consumed `DEDENT`.
//! - In the block form of `if`, `elif`/`else` start a new line. After an
//!   inline branch (`if c: a`) the token stream is `… a NEWLINE else …`, so
//!   `NEWLINE` followed by `elif`/`else` is skipped (`skip_newline_before_else`).
//! - Both branches are required (plan D22); a missing `else` is E0102.
//!
//! ## Where to change things
//!
//! | I want to…                                  | Look at                                               |
//! |---------------------------------------------|-------------------------------------------------------|
//! | add a binary operator / change precedence   | `Parser::expr`, the `match t.kind` table (`Infix`, `l_bp`, `r_bp`) |
//! | add a prefix form (literals, lambdas)       | `Parser::prefix`                                      |
//! | change `if` syntax                          | `if_expr`, `if_block_rest`, `branch`                  |
//! | add postfix forms (calls `f(x)`, `.field`)  | `Parser::expr` loop (currently reports E0009 for `(` `[` `.` `?`) |
//! | add statements (`fn`, `def`)                | `Parser::item` (dispatch on the first token; see `let_binding`) |
//! | change type annotation syntax               | `type_expr`, `type_result`, `type_union`, `type_primary` |
//! | add a construct with an indented body       | call `branch` (or `block`) after its `:`              |
//! | change error recovery                       | `sync_line`, `skip_block`                             |
//!
//! Syntax error codes: E0101 unexpected token, E0102 `if` without `else`,
//! E0103 unclosed `(`, E0104 block ends with a `let`, E0009 construct not
//! supported yet.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `type PResult<T> = Result<T, Failed>` — a type alias; `Failed` is a
//!   marker meaning "an error was already recorded" (*Option and Result*).
//! - `?` to bail out of nested parsing functions on the first error (*Option and Result*).
//! - `Box::new(expr)` to put child nodes on the heap (*Ownership and borrowing*).
//! - nested or-patterns: `TokenKind::Kw(Keyword::And | Keyword::Or)` (*Pattern matching*).
//! - `.clone()` of a token to keep it while advancing (*Ownership and borrowing*).

use crate::ast::{
    BinOp, Binding, CmpLink, CmpOp, Expr, ExprKind, Item, LogicOp, NodeId, Program, TypeExpr,
    TypeKind, UnaryOp,
};
use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::token::{Keyword, Token, TokenKind};

/// Binding power of prefix `-`: binds tighter than `*`, looser than `**`.
const PREFIX_NEG_BP: u8 = 30;
/// Deepest allowed recursion of `Parser::expr` (and of `type_expr`). Every
/// later pass recurses over the tree too, so the limit keeps the compiler
/// from overflowing its stack on pathological input (E0105). Each level of
/// `(1 + (…))` costs two (the group, then the right operand), so this allows
/// about 100 nested parentheses.
pub const MAX_DEPTH: u32 = 200;

/// Binding power of prefix `not`: looser than comparisons, tighter than `and`.
const PREFIX_NOT_BP: u8 = 7;
/// Left binding power of the comparison operators; operands are parsed at
/// `CMP_BP + 1`, so a comparison never swallows another comparison.
const CMP_BP: u8 = 8;

/// What an infix token does (rows of the Pratt table in `Parser::expr`).
enum Infix {
    Arith(BinOp),
    Logic(LogicOp),
    Compare,
}

/// The comparison operator a token spells, if any.
fn cmp_op(kind: &TokenKind) -> Option<CmpOp> {
    Some(match kind {
        TokenKind::EqEq => CmpOp::Eq,
        TokenKind::NotEq => CmpOp::Ne,
        TokenKind::Lt => CmpOp::Lt,
        TokenKind::Le => CmpOp::Le,
        TokenKind::Gt => CmpOp::Gt,
        TokenKind::Ge => CmpOp::Ge,
        _ => return None,
    })
}

/// Parse a token stream (which must end with `EOF`).
pub fn parse(tokens: &[Token]) -> (Program, Vec<Diagnostic>) {
    let mut p = Parser {
        tokens,
        pos: 0,
        diags: Vec::new(),
        next_id: 0,
        depth: 0,
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
    /// Current expression/type nesting depth (see `MAX_DEPTH`).
    depth: u32,
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

    /// item := (let_binding | expr) (NEWLINE | EOF)
    ///
    /// EXTEND: look at the first token here to dispatch to `fn`, `def`, …
    fn item(&mut self) -> PResult<Item> {
        let item = match self.peek().kind {
            TokenKind::Kw(Keyword::Let) => Item::Let(self.let_binding()?),
            _ => Item::Expr(self.expr(0)?),
        };
        self.end_of_line()?;
        Ok(item)
    }

    /// let_binding := 'let' NAME (':' type)? '=' expr   (syntax.md S5)
    fn let_binding(&mut self) -> PResult<Binding> {
        let kw = self.bump();
        let t = self.bump();
        let TokenKind::Ident(name) = t.kind else {
            return Err(self.error(
                Diagnostic::new(
                    "E0101",
                    format!("expected a name after `let`, found {}", t.kind.describe()),
                    t.span,
                )
                .label("expected a name here")
                .with(kw.span, "binding starts here"),
            ));
        };
        let ann = if self.peek().kind == TokenKind::Colon {
            self.bump();
            Some(self.type_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq, "`=`")?;
        let init = self.expr(0)?;
        Ok(Binding {
            id: self.id(),
            name,
            name_span: t.span,
            ann,
            init,
        })
    }

    /// Consume a token of kind `kind`, or report E0101 "expected {what}".
    fn expect(&mut self, kind: &TokenKind, what: &str) -> PResult<Token> {
        if &self.peek().kind == kind {
            return Ok(self.bump());
        }
        let t = self.peek().clone();
        Err(self.error(
            Diagnostic::new(
                "E0101",
                format!("expected {what}, found {}", t.kind.describe()),
                t.span,
            )
            .label(format!("expected {what} here")),
        ))
    }

    // ---- type annotations ----------------------------------------------

    /// type := result ('->' type)?        `->` is right-associative (S4)
    /// result := union ('!' union)?       `!` binds looser than `|` (S10)
    /// union := primary ('|' primary)*
    fn type_expr(&mut self) -> PResult<TypeExpr> {
        self.enter()?;
        let r = self.type_expr_inner();
        self.depth -= 1;
        r
    }

    fn type_expr_inner(&mut self) -> PResult<TypeExpr> {
        let lhs = self.type_result()?;
        if self.peek().kind != TokenKind::Arrow {
            return Ok(lhs);
        }
        self.bump();
        let ret = self.type_expr()?;
        let span = lhs.span.to(ret.span);
        // `(A, B) -> C` lists parameters; `() -> C` has none.
        let params = match lhs.kind {
            TypeKind::Unit => Vec::new(),
            TypeKind::Tuple(ts) => ts,
            _ => vec![lhs],
        };
        Ok(TypeExpr {
            span,
            kind: TypeKind::Fn(params, Box::new(ret)),
        })
    }

    fn type_result(&mut self) -> PResult<TypeExpr> {
        let ok = self.type_union()?;
        if self.peek().kind != TokenKind::Bang {
            return Ok(ok);
        }
        self.bump();
        let err = self.type_union()?;
        Ok(TypeExpr {
            span: ok.span.to(err.span),
            kind: TypeKind::Result(Box::new(ok), Box::new(err)),
        })
    }

    fn type_union(&mut self) -> PResult<TypeExpr> {
        let mut lhs = self.type_primary()?;
        while self.peek().kind == TokenKind::Bar {
            self.bump();
            let rhs = self.type_primary()?;
            lhs = TypeExpr {
                span: lhs.span.to(rhs.span),
                kind: TypeKind::Union(Box::new(lhs), Box::new(rhs)),
            };
        }
        Ok(lhs)
    }

    /// primary := NAME ('[' type (',' type)* ']')? | 'none' | '(' ')'
    ///          | '(' type ')' | '(' type (',' type)+ ','? ')'
    fn type_primary(&mut self) -> PResult<TypeExpr> {
        let t = self.bump();
        match t.kind {
            TokenKind::Ident(name) => {
                if self.peek().kind != TokenKind::LBracket {
                    return Ok(TypeExpr {
                        span: t.span,
                        kind: TypeKind::Name(name),
                    });
                }
                self.bump();
                let (args, close) = self.type_list(&TokenKind::RBracket, "`]`")?;
                Ok(TypeExpr {
                    span: t.span.to(close),
                    kind: TypeKind::Generic(name, args),
                })
            }
            TokenKind::Kw(Keyword::None) => Ok(TypeExpr {
                span: t.span,
                kind: TypeKind::Name("none".into()),
            }),
            TokenKind::LParen => {
                if self.peek().kind == TokenKind::RParen {
                    let close = self.bump();
                    return Ok(TypeExpr {
                        span: t.span.to(close.span),
                        kind: TypeKind::Unit,
                    });
                }
                let first = self.type_expr()?;
                if self.peek().kind == TokenKind::RParen {
                    let close = self.bump();
                    // Grouping: `(int | none)`.
                    return Ok(TypeExpr {
                        span: t.span.to(close.span),
                        kind: first.kind,
                    });
                }
                self.expect(&TokenKind::Comma, "`,` or `)`")?;
                let mut elems = vec![first];
                if self.peek().kind == TokenKind::RParen {
                    // `(A,)`: one-element tuple.
                    let close = self.bump();
                    return Ok(TypeExpr {
                        span: t.span.to(close.span),
                        kind: TypeKind::Tuple(elems),
                    });
                }
                let (rest, close) = self.type_list(&TokenKind::RParen, "`)`")?;
                elems.extend(rest);
                Ok(TypeExpr {
                    span: t.span.to(close),
                    kind: TypeKind::Tuple(elems),
                })
            }
            other => {
                if matches!(other, TokenKind::Newline | TokenKind::Eof) {
                    self.pos -= usize::from(other != TokenKind::Eof);
                }
                Err(self.error(
                    Diagnostic::new(
                        "E0101",
                        format!("expected a type, found {}", other.describe()),
                        t.span,
                    )
                    .label("expected a type here"),
                ))
            }
        }
    }

    /// `type (',' type)* ','? CLOSE`; returns the types and the closer's span.
    fn type_list(
        &mut self,
        close: &TokenKind,
        what: &str,
    ) -> PResult<(Vec<TypeExpr>, crate::span::Span)> {
        let mut out = Vec::new();
        loop {
            out.push(self.type_expr()?);
            if self.peek().kind == TokenKind::Comma {
                self.bump();
                if &self.peek().kind != close {
                    continue;
                }
            }
            let c = self.expect(close, what)?;
            return Ok((out, c.span));
        }
    }

    /// Expect the end of a logical line. A line whose expression ended with
    /// an indented block is already complete: the `NEWLINE` came before the
    /// block's `DEDENT`, which was just consumed.
    fn end_of_line(&mut self) -> PResult<()> {
        match self.peek().kind {
            TokenKind::Newline => {
                self.bump();
                Ok(())
            }
            TokenKind::Eof => Ok(()),
            _ if self.pos > 0 && self.tokens[self.pos - 1].kind == TokenKind::Dedent => Ok(()),
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

    /// Skip to just after the next NEWLINE (or to EOF). If the failed line
    /// ended with an indented block, its DEDENT was just consumed and the
    /// cursor is already at the start of the next line: skip nothing.
    fn sync_line(&mut self) {
        if self.pos > 0 && self.tokens[self.pos - 1].kind == TokenKind::Dedent {
            return;
        }
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
        self.enter()?;
        let r = self.expr_inner(min_bp);
        self.depth -= 1;
        r
    }

    /// Count one nesting level; E0105 beyond `MAX_DEPTH`.
    fn enter(&mut self) -> PResult<()> {
        if self.depth >= MAX_DEPTH {
            let span = self.peek().span;
            return Err(self.error(
                Diagnostic::new("E0105", "expression is nested too deeply", span)
                    .label(format!("more than {MAX_DEPTH} levels of nesting"))
                    .help("split it into several `let` bindings"),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn expr_inner(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.prefix()?;
        loop {
            let t = self.peek().clone();
            // EXTEND: new infix operators get a row here.
            let (op, l_bp, r_bp) = match t.kind {
                TokenKind::Kw(Keyword::Or) => (Infix::Logic(LogicOp::Or), 3, 4),
                TokenKind::Kw(Keyword::And) => (Infix::Logic(LogicOp::And), 5, 6),
                TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge => (Infix::Compare, CMP_BP, CMP_BP + 1),
                TokenKind::Plus => (Infix::Arith(BinOp::Add), 10, 11),
                TokenKind::Minus => (Infix::Arith(BinOp::Sub), 10, 11),
                TokenKind::Star => (Infix::Arith(BinOp::Mul), 20, 21),
                TokenKind::Slash => (Infix::Arith(BinOp::Div), 20, 21),
                TokenKind::SlashSlash => (Infix::Arith(BinOp::IDiv), 20, 21),
                TokenKind::Percent => (Infix::Arith(BinOp::Mod), 20, 21),
                // Right-associative; the right side starts over at prefix level,
                // which admits `2 ** -1`.
                TokenKind::StarStar => (Infix::Arith(BinOp::Pow), 41, 40),
                // Known operators of later milestones: say so clearly
                // instead of a confusing "expected end of line".
                TokenKind::PipeGt
                | TokenKind::Kw(Keyword::In)
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
            if let Infix::Compare = op {
                lhs = self.compare_chain(lhs)?;
                continue;
            }
            self.bump();
            let rhs = self.expr(r_bp)?;
            let span = lhs.span.to(rhs.span);
            let (lhs_box, rhs_box) = (Box::new(lhs), Box::new(rhs));
            let kind = match op {
                Infix::Arith(op) => ExprKind::Binary {
                    op,
                    op_span: t.span,
                    lhs: lhs_box,
                    rhs: rhs_box,
                },
                Infix::Logic(op) => ExprKind::Logic {
                    op,
                    op_span: t.span,
                    lhs: lhs_box,
                    rhs: rhs_box,
                },
                Infix::Compare => unreachable!("handled above"),
            };
            lhs = Expr {
                id: self.id(),
                span,
                kind,
            };
        }
        Ok(lhs)
    }

    /// `first op1 x1 op2 x2 …` while the next token is a comparison: one
    /// `Compare` node for the whole chain (Python semantics).
    fn compare_chain(&mut self, first: Expr) -> PResult<Expr> {
        let mut rest = Vec::new();
        while let Some(op) = cmp_op(&self.peek().kind) {
            let op_span = self.bump().span;
            let rhs = self.expr(CMP_BP + 1)?;
            rest.push(CmpLink { op, op_span, rhs });
        }
        let span = first
            .span
            .to(rest.last().expect("at least one link").rhs.span);
        Ok(Expr {
            id: self.id(),
            span,
            kind: ExprKind::Compare {
                first: Box::new(first),
                rest,
            },
        })
    }

    // ---- conditionals and blocks ---------------------------------------

    /// After `if` (already consumed as `kw`):
    ///
    /// ```text
    /// if_expr := 'if' expr 'then' expr 'else' expr        inline form
    ///          | 'if' if_block_rest                       block form
    /// ```
    ///
    /// The inline form has no `elif`; nest `if … then … else if …` instead.
    fn if_expr(&mut self, kw: Token) -> PResult<Expr> {
        let cond = self.expr(0)?;
        if self.peek().kind != TokenKind::Kw(Keyword::Then) {
            return self.if_block_rest(kw.span, cond);
        }
        self.bump();
        let then_br = self.expr(0)?;
        if self.peek().kind != TokenKind::Kw(Keyword::Else) {
            return Err(self.missing_else(kw.span));
        }
        self.bump();
        let else_br = self.expr(0)?;
        Ok(self.make_if(kw.span, cond, then_br, else_br))
    }

    /// ```text
    /// if_block_rest := ':' branch ( 'elif' expr if_block_rest | 'else' ':' branch )
    /// ```
    ///
    /// `elif c: …` becomes a nested `If` in the else branch, spanning from
    /// the `elif` keyword.
    fn if_block_rest(&mut self, start: Span, cond: Expr) -> PResult<Expr> {
        self.expect(&TokenKind::Colon, "`:` or `then` after the condition")?;
        let then_br = self.branch()?;
        self.skip_newline_before_else();
        let t = self.peek().clone();
        let else_br = match t.kind {
            TokenKind::Kw(Keyword::Elif) => {
                self.bump();
                let c = self.expr(0)?;
                self.if_block_rest(t.span, c)?
            }
            TokenKind::Kw(Keyword::Else) => {
                self.bump();
                self.expect(&TokenKind::Colon, "`:` after `else`")?;
                self.branch()?
            }
            _ => return Err(self.missing_else(start)),
        };
        Ok(self.make_if(start, cond, then_br, else_br))
    }

    fn make_if(&mut self, start: Span, cond: Expr, then_br: Expr, else_br: Expr) -> Expr {
        Expr {
            id: self.id(),
            span: start.to(else_br.span),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_br: Box::new(then_br),
                else_br: Box::new(else_br),
            },
        }
    }

    /// E0102 (plan D22): every `if` needs an `else`, since its value may be used.
    fn missing_else(&mut self, if_span: Span) -> Failed {
        let at = self.peek().span;
        self.error(
            Diagnostic::new("E0102", "`if` without `else`", if_span)
                .label("this `if` has no `else` branch")
                .with(at, "expected `elif` or `else` here")
                .help("an `if` is an expression and needs a value in both cases: add `else: …`"),
        )
    }

    /// After an inline branch (`if c: a`), `elif`/`else` on the next line
    /// arrive as `NEWLINE elif …`; drop that NEWLINE.
    fn skip_newline_before_else(&mut self) {
        let next_is_else = matches!(
            self.tokens.get(self.pos + 1).map(|t| &t.kind),
            Some(TokenKind::Kw(Keyword::Elif | Keyword::Else))
        );
        if self.peek().kind == TokenKind::Newline && next_is_else {
            self.bump();
        }
    }

    /// branch := NEWLINE INDENT block | expr      (the body after a `:`)
    fn branch(&mut self) -> PResult<Expr> {
        let indented = self.peek().kind == TokenKind::Newline
            && self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Indent);
        if !indented {
            return self.expr(0);
        }
        self.bump();
        self.bump();
        self.block()
    }

    /// block := item+ DEDENT   (after NEWLINE INDENT; value = last item)
    ///
    /// Errors inside the block are recovered line by line, so one run can
    /// report several of them.
    fn block(&mut self) -> PResult<Expr> {
        let start = self.peek().span;
        let mut items = Vec::new();
        let mut failed = false;
        loop {
            match self.peek().kind {
                TokenKind::Dedent => {
                    self.bump();
                    break;
                }
                TokenKind::Eof => break,
                TokenKind::Newline => {
                    self.bump();
                }
                _ => match self.item() {
                    Ok(item) => items.push(item),
                    Err(Failed) => {
                        failed = true;
                        self.sync_line();
                    }
                },
            }
        }
        if failed {
            // The errors are recorded; return what parsed so the caller goes
            // on with the line after the block (the pipeline stops anyway).
            return Ok(Expr {
                id: self.id(),
                span: start,
                kind: ExprKind::Block(items),
            });
        }
        let Some(last) = items.last() else {
            let span = self.peek().span;
            return Err(self.error(
                Diagnostic::new("E0101", "expected an indented block", span).label("empty block"),
            ));
        };
        let first_span = match &items[0] {
            Item::Expr(e) => e.span,
            Item::Let(b) => b.name_span,
        };
        let span = match last {
            Item::Expr(e) => first_span.to(e.span),
            Item::Let(b) => {
                // Recorded, but not fatal: the surrounding `if` can still be
                // parsed (the pipeline stops after the parser anyway).
                let span = b.name_span.to(b.init.span);
                self.diags.push(
                    Diagnostic::new("E0104", "block ends with a `let` binding", span)
                        .label("a block's value is its last line, which must be an expression")
                        .help(format!(
                            "add a line with the block's value after this binding, e.g. `{}`",
                            b.name
                        )),
                );
                first_span.to(span)
            }
        };
        Ok(Expr {
            id: self.id(),
            span,
            kind: ExprKind::Block(items),
        })
    }

    /// Prefix position: literals, names, `-x`, `not x`, `if`, `assert`,
    /// parenthesised groups.
    /// EXTEND: lambdas (M6), strings (M7) and list literals (M8) start here.
    fn prefix(&mut self) -> PResult<Expr> {
        let t = self.bump();
        match t.kind {
            TokenKind::Int(n) => Ok(Expr {
                id: self.id(),
                span: t.span,
                kind: ExprKind::Int(n),
            }),
            TokenKind::Float(x) => Ok(Expr {
                id: self.id(),
                span: t.span,
                kind: ExprKind::Float(x),
            }),
            TokenKind::Kw(kw @ (Keyword::True | Keyword::False)) => Ok(Expr {
                id: self.id(),
                span: t.span,
                kind: ExprKind::Bool(kw == Keyword::True),
            }),
            TokenKind::Ident(name) => Ok(Expr {
                id: self.id(),
                span: t.span,
                kind: ExprKind::Name(name),
            }),
            TokenKind::Kw(Keyword::Not) => {
                let operand = self.expr(PREFIX_NOT_BP)?;
                Ok(Expr {
                    id: self.id(),
                    span: t.span.to(operand.span),
                    kind: ExprKind::Not(Box::new(operand)),
                })
            }
            TokenKind::Kw(Keyword::If) => self.if_expr(t),
            // `assert e` (plan D29): an expression of type `()`.
            TokenKind::Kw(Keyword::Assert) => {
                let e = self.expr(0)?;
                Ok(Expr {
                    id: self.id(),
                    span: t.span.to(e.span),
                    kind: ExprKind::Assert(Box::new(e)),
                })
            }
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
            TokenKind::Kw(
                Keyword::Fn
                | Keyword::Def
                | Keyword::Match
                | Keyword::None
                | Keyword::Import
                | Keyword::From
                | Keyword::Test
                | Keyword::Mock,
            )
            | TokenKind::LBracket
            | TokenKind::LBrace => {
                let what = match &t.kind {
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
            parse_codes("f(1)\n[1]\nx in y\nx |> f"),
            ["E0009", "E0009", "E0009", "E0009"]
        );
        assert_eq!(
            parse_codes("let = 1\nlet x 1\nlet y: = 2\n1 + let"),
            ["E0101"; 4]
        );
    }

    // Snapshot: src/snapshots/valo_compiler__parser__tests__logic_and_comparisons.snap
    #[test]
    fn logic_and_comparisons() {
        insta::assert_snapshot!(parse_ok(
            "not a < b and c or d\na or b and c\n0 < x <= 10 != y\nnot not true\n\
             1 + 2 == 3 * 1\nx == y == z\n-1.5 * 2.0 >= 3e2"
        ));
    }

    // Snapshot: src/snapshots/valo_compiler__parser__tests__conditionals_and_blocks.snap
    #[test]
    fn conditionals_and_blocks() {
        insta::assert_snapshot!(parse_ok(
            "if a then 1 else if b then 2 else 3\n\
             if a: 1\nelif b: 2\nelse: 3\n\
             let v = if a:\n    let t = 1\n    t + 1\nelse:\n    if b: 2\n    else: 3\n\
             assert v == 2\nassert not false\nv"
        ));
    }

    #[test]
    fn nesting_limit() {
        // At the limit: fine (and must not overflow the 2 MiB test-thread
        // stack in later passes either; see codegen's `deep_nesting_compiles`).
        let n = MAX_DEPTH as usize / 2 - 1;
        let ok = format!("{}1{}", "(1 + ".repeat(n), ")".repeat(n));
        assert!(parse_codes(&ok).is_empty());
        let deep = format!("{}1{}", "(1 + ".repeat(n + 2), ")".repeat(n + 2));
        assert_eq!(parse_codes(&deep), ["E0105"]);
        let deep_ty = format!("let x: {}int{} = 1", "(".repeat(250), ")".repeat(250));
        assert_eq!(parse_codes(&deep_ty), ["E0105"]);
    }

    #[test]
    fn conditional_errors() {
        // E0102: missing else (both forms); the next line still parses.
        assert_eq!(parse_codes("if a then 1\n1 +"), ["E0102", "E0101"]);
        assert_eq!(parse_codes("if a: 1\n2\n1 +"), ["E0102", "E0101"]);
        assert_eq!(parse_codes("if a:\n    1\n2 2"), ["E0102", "E0101"]);
        // E0104: block ends with a binding; recovery continues after it.
        assert_eq!(
            parse_codes("let v = if a:\n    let t = 1\nelse: 2\n1 +"),
            ["E0104", "E0101"]
        );
        // Errors inside a block are reported line by line.
        assert_eq!(
            parse_codes("if a:\n    1 +\n    2 2\n    3\nelse: 4\n5 5"),
            ["E0101", "E0101", "E0101"]
        );
        assert_eq!(parse_codes("if a 1 else 2"), ["E0101"]);
        assert_eq!(parse_codes("else: 1"), ["E0101"]);
    }

    // Snapshot: src/snapshots/valo_compiler__parser__tests__let_and_annotations.snap
    #[test]
    fn let_and_annotations() {
        insta::assert_snapshot!(parse_ok(
            "let a = 1\nlet b: int = a + 2\nlet xs: list[int] = b\nlet r: int | none ! error = a\n\
             let f: (int, int) -> int = a\nlet g: () -> str = a\nlet h: int -> int -> int = a\n\
             let t: (int, dict[str, (int,)]) = a\na * b"
        ));
        assert_eq!(parse_codes("(1 2)"), ["E0101"]);
    }
}
