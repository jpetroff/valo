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
//! ## Structure (M7)
//!
//! ```text
//! Program { items: Vec<Item> }
//! Item::Expr(Expr)                       an expression line
//!     ::Let(Binding)                     `let name (: type)? = init`
//!     ::Fn(FnDef)                        `fn name[T](p: type, …) -> type: body`
//! Binding { id, name, name_span, ann: Option<TypeExpr>, init: Expr }
//! FnDef { id, name, type_params, params: Vec<Param>, ret, body: Expr, span }
//! Param { id, name, span, ann: Option<TypeExpr>, default: Option<Expr> }
//! Expr { id: NodeId, span: Span, kind: ExprKind }
//! ExprKind::Int(i64) | Float(f64) | Bool(bool) | Unit
//!         ::Name(String)                 a reference to a binding
//!         ::Unary  { op: UnaryOp, operand }          `-x`
//!         ::Not(operand)                             `not x`
//!         ::Binary { op: BinOp, op_span, lhs, rhs }  arithmetic
//!         ::Logic  { op: LogicOp, op_span, lhs, rhs } `and` / `or`
//!         ::Compare { first, rest: Vec<CmpLink> }    `a < b <= c` (one node per chain)
//!         ::If { cond, then_br, else_br }            both forms; `elif` = nested If
//!         ::Block(Vec<Item>)                         indented block; last item is an Expr
//!         ::Assert(expr)                             `assert e`, value `()`
//!         ::Call { callee, args: Vec<Arg>, piped }   `f(a, k=b)`
//!         ::Pipe { lhs, rhs }                        `x |> f(a)`; rewritten to a
//!                                                    `Call` by `desugar.rs`
//!         ::Lambda(Lambda{params, body, partial_of}) `x => e`, `(a, b) => e` (M6)
//!         ::Str(String)                              `"text"` (M7)
//!         ::Interp(Vec<StrPart>)                     `"a {x} b"`: Text / Expr parts (M7)
//! Arg::Positional(Expr) | Keyword { name, value } | Placeholder (`_`, M6)
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
    /// The next unused `NodeId` (every id in the tree is smaller), so
    /// `desugar.rs` can create new nodes.
    pub next_id: u32,
}

/// A top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// An expression evaluated for its effect; its value is discarded
    /// (shown with `valo run --echo`).
    Expr(Expr),
    /// An immutable binding (syntax.md S5).
    Let(Binding),
    /// A named function definition (syntax.md S2).
    Fn(FnDef),
}

/// `fn name[T, …](params) -> ret` followed by `: body` or `= expr`.
#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    /// Node id; the resolver records the function's global slot under it.
    pub id: NodeId,
    /// Function name.
    pub name: String,
    /// Span of the name.
    pub name_span: Span,
    /// Declared type variables (S20), with their spans.
    pub type_params: Vec<(String, Span)>,
    /// Parameters in order.
    pub params: Vec<Param>,
    /// Return type. Required (S13); `None` only after E0410 was reported.
    pub ret: Option<TypeExpr>,
    /// Body; its value is the function's result.
    pub body: Expr,
    /// From `fn` to the end of the body.
    pub span: Span,
}

/// One function parameter `name: type (= default)?`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// Node id; the resolver records the parameter's local binding under it.
    pub id: NodeId,
    /// Name.
    pub name: String,
    /// Span of the name.
    pub span: Span,
    /// Type. Required (S13); `None` only after E0410 was reported.
    pub ann: Option<TypeExpr>,
    /// Default value (keyword-only parameter, S19; M4b).
    pub default: Option<Expr>,
}

/// One argument of a call.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// `f(x)`.
    Positional(Expr),
    /// `f(name=x)` (S19; M4b).
    Keyword {
        /// Parameter name.
        name: String,
        /// Span of the name.
        name_span: Span,
        /// Value.
        value: Expr,
    },
    /// `f(_)`: placeholder for partial application (S1; M6).
    Placeholder(Span),
}

/// An anonymous function (S3). The enclosing `Expr`'s id keys the
/// resolver's capture list for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lambda {
    /// Parameters (annotations optional, never a default).
    pub params: Vec<Param>,
    /// Body; its value is the result.
    pub body: Expr,
    /// Set by `desugar.rs` when the lambda is a `_` partial of the named
    /// function (or `<expr>`); code generation names it `<partial f@l:c>`
    /// (plan D23). `None` for lambdas written in the source.
    pub partial_of: Option<String>,
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
    /// The unit value `()`.
    Unit,
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
    /// A call `callee(args)` (syntax.md S1). `piped` marks calls made from a
    /// pipe by `desugar.rs`: the argument at that index (the piped value) is
    /// evaluated before the callee and the other arguments (Q17).
    Call {
        /// The function.
        callee: Box<Expr>,
        /// Arguments in source order.
        args: Vec<Arg>,
        /// Built from `lhs |> …`: index of the piped argument.
        piped: Option<usize>,
    },
    /// A lambda `x => e`, `(a, b: int) => e`, `() => e` (syntax.md S3; M6).
    Lambda(Box<Lambda>),
    /// A string literal without interpolation (escapes decoded; M7).
    Str(String),
    /// An interpolated string `"a {x} b"` (S12, M7): text and expressions
    /// in order; each expression is converted to text when it runs.
    Interp(Vec<StrPart>),
    /// `lhs |> rhs` (S1). Only the parser produces it; `desugar.rs`
    /// rewrites it into a `Call` before name resolution.
    Pipe {
        /// Span of the `|>` token.
        op_span: Span,
        /// The value fed in.
        lhs: Box<Expr>,
        /// A call (the value becomes its first argument) or any function.
        rhs: Box<Expr>,
    },
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

/// A piece of an interpolated string.
#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    /// Literal text.
    Text(String),
    /// `{expr}`.
    Expr(Expr),
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
            ExprKind::Unit => f.write_str("()"),
            ExprKind::Name(n) => f.write_str(n),
            ExprKind::Call {
                callee,
                args,
                piped,
            } => {
                write!(
                    f,
                    "({} {callee}",
                    if piped.is_some() { "pcall" } else { "call" }
                )?;
                for a in args {
                    write!(f, " {a}")?;
                }
                f.write_str(")")
            }
            ExprKind::Pipe { lhs, rhs, .. } => write!(f, "(|> {lhs} {rhs})"),
            ExprKind::Str(text) => write!(f, "{text:?}"),
            ExprKind::Interp(parts) => {
                f.write_str("(interp")?;
                for p in parts {
                    match p {
                        StrPart::Text(t) => write!(f, " {t:?}")?,
                        StrPart::Expr(e) => write!(f, " {e}")?,
                    }
                }
                f.write_str(")")
            }
            ExprKind::Lambda(l) => {
                f.write_str("(lambda ")?;
                if let Some(n) = &l.partial_of {
                    write!(f, "partial:{n} ")?;
                }
                f.write_str("(")?;
                for (i, p) in l.params.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") {})", l.body)
            }
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

impl fmt::Display for Arg {
    /// `x`, `name=x` or `_`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arg::Positional(e) => write!(f, "{e}"),
            Arg::Keyword { name, value, .. } => write!(f, "{name}={value}"),
            Arg::Placeholder(_) => f.write_str("_"),
        }
    }
}

impl fmt::Display for Param {
    /// `(x : int)`, `(x : int = 1)`, `(x)` when the type is missing.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}", self.name)?;
        if let Some(t) = &self.ann {
            write!(f, " : {t}")?;
        }
        if let Some(d) = &self.default {
            write!(f, " = {d}")?;
        }
        f.write_str(")")
    }
}

impl fmt::Display for FnDef {
    /// `(fn name [T] ((x : int)) -> int body)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(fn {}", self.name)?;
        if !self.type_params.is_empty() {
            let names: Vec<&str> = self.type_params.iter().map(|(n, _)| n.as_str()).collect();
            write!(f, " [{}]", names.join(" "))?;
        }
        f.write_str(" (")?;
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{p}")?;
        }
        f.write_str(")")?;
        if let Some(r) = &self.ret {
            write!(f, " -> {r}")?;
        }
        write!(f, " {})", self.body)
    }
}

impl fmt::Display for Item {
    /// `(expr …)`, `(let name : type init)` or `(fn …)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Expr(e) => write!(f, "(expr {e})"),
            Item::Fn(d) => write!(f, "{d}"),
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
