//! # Abstract syntax tree (`ast.rs`)
//!
//! **What this file is:** the tree-shaped representation of a program after
//! parsing. `1 + 2 * 3` becomes `Binary(+, Int 1, Binary(*, Int 2, Int 3))`:
//! parentheses, spaces and comments are gone; operator precedence is now
//! explicit in the shape of the tree.
//!
//! **Pipeline position:** built by `parser.rs`, read by `resolve.rs` and
//! `codegen.rs`. Printed as S-expressions by `valo check --dump ast`.
//!
//! **Textbook chapter:** `docs/04-syntax-analysis.md`.
//!
//! ## Structure (M1)
//!
//! ```text
//! Program { items: Vec<Item> }
//! Item::Expr(Expr)                       one top-level expression per line
//! Expr { id: NodeId, span: Span, kind: ExprKind }
//! ExprKind::Int(i64)
//!         ::Unary  { op: UnaryOp, operand: Box<Expr> }
//!         ::Binary { op: BinOp, op_span, lhs: Box<Expr>, rhs: Box<Expr> }
//! ```
//!
//! ## Where to change things
//!
//! | I want to…                              | Look at                                                       |
//! |-----------------------------------------|---------------------------------------------------------------|
//! | add an expression form (names, `if`, calls) | new variant in `ExprKind`; then `impl fmt::Display for Expr` (S-expr), `parser.rs`, `codegen.rs` — the Rust compiler lists every `match` you must extend |
//! | add a top-level form (`let`, `fn`, `def`)   | new variant in `enum Item`; then `dump`, `parser.rs` (`Parser::item`), `codegen.rs` (`compile`) |
//! | add an operator                          | `BinOp` / `UnaryOp` + `BinOp::as_str`                         |
//!
//! It is a plain `Box` tree (plan §1.1). Every expression carries a `NodeId`,
//! so later passes (resolve, type checks) can attach side tables keyed by id,
//! and a `Span` for diagnostics and the bytecode span table.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `Box<Expr>` — a heap pointer; needed because a type cannot contain
//!   itself directly (it would have infinite size) (*Ownership and borrowing*).
//! - struct-like enum variants `Binary { op, lhs, rhs }` (*Enums*).
//! - newtype `struct NodeId(pub u32)` (*Structs*).

use std::fmt::{self, Write as _};

use crate::span::Span;

/// Unique id of an AST node within one program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// A whole source file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    /// Top-level items in source order.
    pub items: Vec<Item>,
}

/// A top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// An expression evaluated for its effect; its value is discarded
    /// (shown with `valo run --echo`).
    Expr(Expr),
}

/// An expression node.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// Node id.
    pub id: NodeId,
    /// Source range (includes surrounding parentheses).
    pub span: Span,
    /// What kind of expression.
    pub kind: ExprKind,
}

/// Expression kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Integer literal.
    Int(i64),
    /// Prefix operator.
    Unary {
        /// Operator.
        op: UnaryOp,
        /// Operand.
        operand: Box<Expr>,
    },
    /// Infix operator.
    Binary {
        /// Operator.
        op: BinOp,
        /// Span of the operator token.
        op_span: Span,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
}

/// Prefix operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-x`
    Neg,
}

/// Infix operators (syntax.md S11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/` (true division, float result; M3)
    Div,
    /// `//`
    IDiv,
    /// `%`
    Mod,
    /// `**`
    Pow,
}

impl BinOp {
    /// Source spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::IDiv => "//",
            BinOp::Mod => "%",
            BinOp::Pow => "**",
        }
    }
}

impl fmt::Display for Expr {
    /// S-expression form used by `--dump ast` and snapshots.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Int(n) => write!(f, "{n}"),
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            } => write!(f, "(neg {operand})"),
            ExprKind::Binary { op, lhs, rhs, .. } => write!(f, "({} {lhs} {rhs})", op.as_str()),
        }
    }
}

/// One line per item, for `--dump ast`.
pub fn dump(p: &Program) -> String {
    let mut out = String::new();
    for item in &p.items {
        match item {
            Item::Expr(e) => {
                let _ = writeln!(out, "(expr {e})");
            }
        }
    }
    out
}
