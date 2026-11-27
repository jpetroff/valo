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
//! ## Structure (M3)
//!
//! ```text
//! Program { items: Vec<Item> }
//! Item::Expr(Expr)                       an expression line
//!     ::Let(Binding)                     `let name (: type)? = init`
//! Binding { id, name, name_span, ann: Option<TypeExpr>, init: Expr }
//! Expr { id: NodeId, span: Span, kind: ExprKind }
//! ExprKind::Int(i64) | Float(f64) | Bool(bool)
//!         ::Name(String)                 a reference to a binding
//!         ::Unary  { op: UnaryOp, operand }          `-x`
//!         ::Not(operand)                             `not x`
//!         ::Binary { op: BinOp, op_span, lhs, rhs }  arithmetic
//!         ::Logic  { op: LogicOp, op_span, lhs, rhs } `and` / `or`
//!         ::Compare { first, rest: Vec<CmpLink> }    `a < b <= c` (one node per chain)
//!         ::If { cond, then_br, else_br }            both forms; `elif` = nested If
//!         ::Block(Vec<Item>)                         indented block; last item is an Expr
//!         ::Assert(expr)                             `assert e`, value `()`
//! TypeExpr { span, kind: TypeKind }      annotations (checked from M10)
//! ```
//!
//! ## Where to change things
//!
//! | I want to…                              | Look at                                                       |
//! |-----------------------------------------|---------------------------------------------------------------|
//! | add an expression form (names, `if`, calls) | new variant in `ExprKind`; then `impl fmt::Display for Expr` (S-expr), `parser.rs`, `codegen.rs` — the Rust compiler lists every `match` you must extend |
//! | add a top-level form (`fn`, `def`)       | new variant in `enum Item`; then `dump`, `parser.rs` (`Parser::item`), `resolve.rs`, `codegen.rs` (`compile`) |
//! | add a type form                          | `TypeKind` + its `Display`, `Parser::type_expr`  |
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
    /// An immutable binding (syntax.md S5).
    Let(Binding),
}

/// `let name (: type)? = init`.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    /// Node id; the resolver records which slot the binding defines.
    pub id: NodeId,
    /// Bound name; `_` binds nothing (the value is discarded).
    pub name: String,
    /// Span of the name.
    pub name_span: Span,
    /// Optional annotation (parsed and stored; checked at runtime from M10).
    pub ann: Option<TypeExpr>,
    /// Initialiser.
    pub init: Expr,
}

/// A type annotation (syntax.md S4, S9, S10, S20).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    /// Source range.
    pub span: Span,
    /// Which form.
    pub kind: TypeKind,
}

/// Type annotation forms.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// `int`, `str`, `none`, a type variable `T`, …
    Name(String),
    /// `list[int]`, `dict[str, int]`.
    Generic(String, Vec<TypeExpr>),
    /// `()`.
    Unit,
    /// `(A, B)`.
    Tuple(Vec<TypeExpr>),
    /// `A | B` (optional: `T | none`).
    Union(Box<TypeExpr>, Box<TypeExpr>),
    /// `T ! E` (result).
    Result(Box<TypeExpr>, Box<TypeExpr>),
    /// `A -> B`, `(A, B) -> C`, `() -> C`.
    Fn(Vec<TypeExpr>, Box<TypeExpr>),
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
    /// Float literal.
    Float(f64),
    /// `true` / `false`.
    Bool(bool),
    /// A name reference (resolved by `resolve.rs`).
    Name(String),
    /// `not x` (bool only).
    Not(Box<Expr>),
    /// Short-circuit `and` / `or` (bool operands only, syntax.md S11).
    Logic {
        /// Operator.
        op: LogicOp,
        /// Span of the operator keyword.
        op_span: Span,
        /// Left operand (always evaluated).
        lhs: Box<Expr>,
        /// Right operand (evaluated only if needed).
        rhs: Box<Expr>,
    },
    /// A comparison chain `first op1 x1 op2 x2 …` (Python): every operand
    /// is evaluated at most once, and evaluation stops at the first false link.
    Compare {
        /// Leftmost operand.
        first: Box<Expr>,
        /// One entry per operator (at least one).
        rest: Vec<CmpLink>,
    },
    /// Conditional expression (both forms of syntax.md S6; `elif` is a
    /// nested `If` in `else_br`). Both branches are required (D22).
    If {
        /// Condition (must be a bool at runtime).
        cond: Box<Expr>,
        /// Value when true.
        then_br: Box<Expr>,
        /// Value when false.
        else_br: Box<Expr>,
    },
    /// An indented block: bindings and expressions; its value is the last
    /// item, which the parser guarantees is an `Item::Expr` (E0104).
    Block(Vec<Item>),
    /// `assert e`: runtime error R0701 if `e` is false; value `()` (D29).
    Assert(Box<Expr>),
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

/// One `op operand` step of a comparison chain.
#[derive(Debug, Clone, PartialEq)]
pub struct CmpLink {
    /// Operator.
    pub op: CmpOp,
    /// Span of the operator token.
    pub op_span: Span,
    /// Right operand.
    pub rhs: Expr,
}

/// Comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CmpOp {
    /// Source spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

/// Short-circuit logic operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicOp {
    /// `and`
    And,
    /// `or`
    Or,
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
            ExprKind::Float(x) => write!(f, "{x:?}"),
            ExprKind::Bool(b) => write!(f, "{b}"),
            ExprKind::Name(n) => f.write_str(n),
            ExprKind::Not(e) => write!(f, "(not {e})"),
            ExprKind::Logic { op, lhs, rhs, .. } => {
                let op = match op {
                    LogicOp::And => "and",
                    LogicOp::Or => "or",
                };
                write!(f, "({op} {lhs} {rhs})")
            }
            // One link prints like a binary operator; longer chains list
            // operands and operators in source order.
            ExprKind::Compare { first, rest } if rest.len() == 1 => {
                write!(f, "({} {first} {})", rest[0].op.as_str(), rest[0].rhs)
            }
            ExprKind::Compare { first, rest } => {
                write!(f, "(chain {first}")?;
                for l in rest {
                    write!(f, " {} {}", l.op.as_str(), l.rhs)?;
                }
                f.write_str(")")
            }
            ExprKind::If {
                cond,
                then_br,
                else_br,
            } => write!(f, "(if {cond} {then_br} {else_br})"),
            ExprKind::Block(items) => {
                f.write_str("(block")?;
                for i in items {
                    write!(f, " {i}")?;
                }
                f.write_str(")")
            }
            ExprKind::Assert(e) => write!(f, "(assert {e})"),
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            } => write!(f, "(neg {operand})"),
            ExprKind::Binary { op, lhs, rhs, .. } => write!(f, "({} {lhs} {rhs})", op.as_str()),
        }
    }
}

impl fmt::Display for TypeExpr {
    /// S-expression form: `(list int)`, `(| int none)`, `(-> (int int) int)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TypeKind::Name(n) => f.write_str(n),
            TypeKind::Generic(n, args) => {
                write!(f, "({n}")?;
                for a in args {
                    write!(f, " {a}")?;
                }
                f.write_str(")")
            }
            TypeKind::Unit => f.write_str("()"),
            TypeKind::Tuple(ts) => {
                f.write_str("(tuple")?;
                for t in ts {
                    write!(f, " {t}")?;
                }
                f.write_str(")")
            }
            TypeKind::Union(a, b) => write!(f, "(| {a} {b})"),
            TypeKind::Result(a, b) => write!(f, "(! {a} {b})"),
            TypeKind::Fn(params, ret) => {
                f.write_str("(-> (")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") {ret})")
            }
        }
    }
}

impl fmt::Display for Item {
    /// `(expr …)` or `(let name : type init)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Expr(e) => write!(f, "(expr {e})"),
            Item::Let(b) => match &b.ann {
                Some(t) => write!(f, "(let {} : {t} {})", b.name, b.init),
                None => write!(f, "(let {} {})", b.name, b.init),
            },
        }
    }
}

/// One line per item, for `--dump ast`.
pub fn dump(p: &Program) -> String {
    let mut out = String::new();
    for item in &p.items {
        let _ = writeln!(out, "{item}");
    }
    out
}
