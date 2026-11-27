//! # Desugaring (`desugar.rs`)
//!
//! **What this file is:** a small pass that rewrites convenience syntax into
//! the core forms the later passes understand, so name resolution and code
//! generation never see it. Today that is only the pipe (plan D24):
//!
//! ```text
//! x |> f(a, b)   →   f(x, a, b)     the value becomes the FIRST argument
//! x |> f         →   f(x)           any other right side is called with x
//! ```
//!
//! The resulting `Call` has `piped: true`, which tells code generation to
//! evaluate `x` before the callee (Q17), and keeps the pipe's span, so errors
//! point at the whole stage.
//!
//! **Pipeline position:** `ast::Program` from the parser in → rewritten
//! `ast::Program` out → `resolve.rs`. `--dump ast` shows the program *before*
//! this pass. Entry point: `desugar`.
//!
//! **Textbook chapter:** `docs/04-syntax-analysis.md` (section *Desugaring*).
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                     |
//! |----------------------------------------------|-----------------------------|
//! | rewrite another construct (`_` partials, M6; comprehensions, M8) | `expr`, after the children are rewritten |
//! | visit a new expression kind                  | `children`                  |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `&mut` traversal that edits the tree in place (*Ownership and borrowing*).
//! - `std::mem::replace` to move a value out of a `&mut` place (*Ownership and borrowing*).

use crate::ast::{Arg, Expr, ExprKind, Item, Program};

/// Rewrite every pipe in `program` into a call.
pub fn desugar(mut program: Program) -> Program {
    for item in &mut program.items {
        self::item(item);
    }
    program
}

fn item(item: &mut Item) {
    match item {
        Item::Expr(e) => expr(e),
        Item::Let(b) => expr(&mut b.init),
        Item::Fn(d) => {
            for p in &mut d.params {
                if let Some(e) = &mut p.default {
                    expr(e);
                }
            }
            expr(&mut d.body);
        }
    }
}

/// Rewrite `e`: children first (so `a |> f |> g` works from the inside out),
/// then `e` itself.
fn expr(e: &mut Expr) {
    children(e);
    if !matches!(e.kind, ExprKind::Pipe { .. }) {
        return;
    }
    // Move the pipe out of the tree, leaving a placeholder for a moment.
    let ExprKind::Pipe { lhs, rhs, .. } = std::mem::replace(&mut e.kind, ExprKind::Unit) else {
        unreachable!("checked above");
    };
    let rhs = *rhs;
    e.kind = match rhs.kind {
        // `x |> f(a)` → `f(x, a)`. A call that is itself a rewritten pipe
        // (`x |> (y |> f)`) is a computed function instead: call it with x.
        ExprKind::Call {
            callee,
            mut args,
            piped: false,
        } => {
            args.insert(0, Arg::Positional(*lhs));
            ExprKind::Call {
                callee,
                args,
                piped: true,
            }
        }
        // `x |> f` → `f(x)`.
        kind => ExprKind::Call {
            callee: Box::new(Expr {
                id: rhs.id,
                span: rhs.span,
                kind,
            }),
            args: vec![Arg::Positional(*lhs)],
            piped: true,
        },
    };
}

/// Recurse into the sub-expressions of `e`.
/// EXTEND: every new `ExprKind` with children needs an arm.
fn children(e: &mut Expr) {
    match &mut e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Name(_) => {}
        ExprKind::Not(x) | ExprKind::Assert(x) | ExprKind::Unary { operand: x, .. } => expr(x),
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::Logic { lhs, rhs, .. }
        | ExprKind::Pipe { lhs, rhs, .. } => {
            expr(lhs);
            expr(rhs);
        }
        ExprKind::Compare { first, rest } => {
            expr(first);
            for link in rest {
                expr(&mut link.rhs);
            }
        }
        ExprKind::If {
            cond,
            then_br,
            else_br,
        } => {
            expr(cond);
            expr(then_br);
            expr(else_br);
        }
        ExprKind::Block(items) => {
            for i in items {
                item(i);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            expr(callee);
            for a in args {
                match a {
                    Arg::Positional(x) | Arg::Keyword { value: x, .. } => expr(x),
                    Arg::Placeholder(_) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::span::SourceMap;
    use crate::{ast, parse_source};

    fn desugared(src: &str) -> String {
        let sm = SourceMap::new("t.vl", src);
        ast::dump(&super::desugar(parse_source(&sm).expect("parses")))
    }

    #[test]
    fn pipes_become_calls() {
        assert_eq!(desugared("x |> f"), "(expr (pcall f x))\n");
        assert_eq!(desugared("x |> f(a)"), "(expr (pcall f x a))\n");
        assert_eq!(
            desugared("x |> f(a)\n  |> g\n  |> h(1, 2)"),
            "(expr (pcall h (pcall g (pcall f x a)) 1 2))\n"
        );
        assert_eq!(desugared("x |> (y |> f)"), "(expr (pcall (pcall f y) x))\n");
        assert_eq!(desugared("a + 1 |> f"), "(expr (pcall f (+ a 1)))\n");
        assert_eq!(desugared("x |> f(k=1)"), "(expr (pcall f x k=1))\n");
    }
}
