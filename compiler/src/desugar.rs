//! # Desugaring (`desugar.rs`)
//!
//! **What this file is:** a small pass that rewrites convenience syntax into
//! the core forms the later passes understand, so name resolution and code
//! generation never see it: the pipe (plan D24) and `_` partial application
//! (plan D6, M6).
//!
//! ```text
//! x |> f(a, b)   →   f(x, a, b)     the value becomes the FIRST argument
//! x |> f(a, _)   →   f(a, x)        … or fills the first `_` (no lambda)
//! x |> f         →   f(x)           any other right side is called with x
//!
//! f(a, _, k=b)   →   block                       a `_` partial: fixed
//!                      let $a0 = a                arguments are evaluated
//!                      let $a1 = b                once, in source order,
//!                      ($p0) => f($a0, $p0, k=$a1)   when the partial is built
//! ```
//!
//! A piped `Call` records the index of the piped argument (`piped`), which
//! tells code generation to evaluate it before the callee (Q17), and keeps
//! the pipe's span, so errors point at the whole stage. A partial whose
//! callee is not a plain name binds the callee to `$c` first (after a piped
//! value, before the other arguments); a plain name stays in the call so the
//! resolver can still check its arity and keywords. The lambda is marked
//! `partial_of` (its display name becomes `<partial f@l:c>`, D23). Generated
//! names start with `$`, which source code cannot spell, so they never
//! clash with user names; every generated node gets the call's span.
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
//! | rewrite another construct (comprehensions, M8) | `Desugar::expr`, after the children are rewritten |
//! | change the pipe rewrite                      | `Desugar::pipe`             |
//! | change the partial rewrite                   | `Desugar::partial`          |
//! | visit a new expression kind                  | `Desugar::children`         |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `&mut` traversal that edits the tree in place (*Ownership and borrowing*).
//! - `std::mem::replace` to move a value out of a `&mut` place (*Ownership and borrowing*).

use crate::ast::{Arg, Binding, Expr, ExprKind, Item, Lambda, NodeId, Param, Program};
use crate::span::Span;

/// Rewrite every pipe in `program` into a call, and every `_` partial into
/// a lambda.
pub fn desugar(mut program: Program) -> Program {
    let mut d = Desugar {
        next_id: program.next_id,
    };
    for item in &mut program.items {
        d.item(item);
    }
    program.next_id = d.next_id;
    program
}

/// Pass state: the next fresh `NodeId`.
struct Desugar {
    next_id: u32,
}

/// Whether a call's arguments contain a `_` placeholder.
fn has_placeholder(args: &[Arg]) -> bool {
    args.iter().any(|a| matches!(a, Arg::Placeholder(_)))
}

impl Desugar {
    /// A fresh node with `span`.
    fn node(&mut self, span: Span, kind: ExprKind) -> Expr {
        Expr {
            id: self.id(),
            span,
            kind,
        }
    }

    fn id(&mut self) -> NodeId {
        self.next_id += 1;
        NodeId(self.next_id - 1)
    }

    fn item(&mut self, item: &mut Item) {
        match item {
            Item::Expr(e) => self.expr(e),
            Item::Let(b) => self.expr(&mut b.init),
            Item::Fn(d) => {
                for p in &mut d.params {
                    if let Some(e) = &mut p.default {
                        self.expr(e);
                    }
                }
                self.expr(&mut d.body);
            }
        }
    }

    /// Rewrite `e`: children first (so `a |> f |> g` works from the inside
    /// out), then `e` itself. A pipe handles its right side itself, because
    /// a `_` there is filled by the piped value, not turned into a lambda.
    fn expr(&mut self, e: &mut Expr) {
        if matches!(e.kind, ExprKind::Pipe { .. }) {
            return self.pipe(e);
        }
        self.children(e);
        if let ExprKind::Call { args, .. } = &e.kind {
            if has_placeholder(args) {
                self.partial(e);
            }
        }
    }

    /// `lhs |> rhs` → a call (D24).
    fn pipe(&mut self, e: &mut Expr) {
        // Move the pipe out of the tree, leaving a placeholder for a moment.
        let ExprKind::Pipe { mut lhs, rhs, .. } = std::mem::replace(&mut e.kind, ExprKind::Unit)
        else {
            unreachable!("checked by the caller");
        };
        self.expr(&mut lhs);
        let mut rhs = *rhs;
        match rhs.kind {
            // `x |> f(a)` → `f(x, a)`; `x |> f(a, _)` → `f(a, x)`.
            ExprKind::Call {
                mut callee,
                mut args,
                piped: None,
            } => {
                self.expr(&mut callee);
                for a in &mut args {
                    self.arg(a);
                }
                let slot = args.iter().position(|a| matches!(a, Arg::Placeholder(_)));
                let piped = match slot {
                    Some(k) => {
                        args[k] = Arg::Positional(*lhs);
                        k
                    }
                    None => {
                        args.insert(0, Arg::Positional(*lhs));
                        0
                    }
                };
                e.kind = ExprKind::Call {
                    callee,
                    args,
                    piped: Some(piped),
                };
                // More `_` left (`x |> f(_, _)`): a partial with `x` fixed.
                if let ExprKind::Call { args, .. } = &e.kind {
                    if has_placeholder(args) {
                        self.partial(e);
                    }
                }
            }
            // `x |> f` → `f(x)`. A computed function (`x |> (y |> f)`) is
            // rewritten first, then called with `x`.
            _ => {
                self.expr(&mut rhs);
                e.kind = ExprKind::Call {
                    callee: Box::new(rhs),
                    args: vec![Arg::Positional(*lhs)],
                    piped: Some(0),
                };
            }
        }
    }

    /// `f(a, _, k=b)` (children already rewritten) → a block that binds the
    /// fixed arguments to temporaries and ends with a lambda (D6).
    fn partial(&mut self, e: &mut Expr) {
        let span = e.span;
        let ExprKind::Call {
            callee,
            args,
            piped,
        } = std::mem::replace(&mut e.kind, ExprKind::Unit)
        else {
            unreachable!("checked by the caller");
        };
        let mut lets: Vec<Item> = Vec::new();
        let mut params: Vec<Param> = Vec::new();
        let mut new_args: Vec<Arg> = Vec::with_capacity(args.len());
        // Fixed values in evaluation order (index into `new_args` for the
        // piped one, which goes first, Q17).
        let mut fixed: Vec<(usize, String, Expr)> = Vec::new();
        for (i, a) in args.into_iter().enumerate() {
            match a {
                Arg::Placeholder(p_span) => {
                    let name = format!("$p{}", params.len());
                    params.push(Param {
                        id: self.id(),
                        name: name.clone(),
                        span: p_span,
                        ann: None,
                        default: None,
                    });
                    new_args.push(Arg::Positional(self.node(span, ExprKind::Name(name))));
                }
                Arg::Positional(v) => {
                    let name = format!("$a{}", fixed.len());
                    fixed.push((i, name.clone(), v));
                    new_args.push(Arg::Positional(self.node(span, ExprKind::Name(name))));
                }
                Arg::Keyword {
                    name: kw,
                    name_span,
                    value,
                } => {
                    let name = format!("$a{}", fixed.len());
                    fixed.push((i, name.clone(), value));
                    new_args.push(Arg::Keyword {
                        name: kw,
                        name_span,
                        value: self.node(span, ExprKind::Name(name)),
                    });
                }
            }
        }
        let bind = |d: &mut Desugar, name: String, init: Expr| {
            Item::Let(Binding {
                id: d.id(),
                name,
                name_span: span,
                ann: None,
                init,
            })
        };
        // The piped value is evaluated first (Q17).
        if let Some(k) = piped {
            if let Some(pos) = fixed.iter().position(|(i, ..)| *i == k) {
                let (_, name, v) = fixed.remove(pos);
                lets.push(bind(self, name, v));
            }
        }
        let (callee, partial_of) = match &callee.kind {
            ExprKind::Name(n) => {
                let n = n.clone();
                (*callee, n)
            }
            _ => {
                let c = self.node(span, ExprKind::Name("$c".into()));
                lets.push(bind(self, "$c".into(), *callee));
                (c, "<expr>".to_string())
            }
        };
        for (_, name, v) in fixed {
            lets.push(bind(self, name, v));
        }
        let call = self.node(
            span,
            ExprKind::Call {
                callee: Box::new(callee),
                args: new_args,
                piped: None,
            },
        );
        let lambda = ExprKind::Lambda(Box::new(Lambda {
            params,
            body: call,
            partial_of: Some(partial_of),
        }));
        if lets.is_empty() {
            e.kind = lambda;
        } else {
            lets.push(Item::Expr(self.node(span, lambda)));
            e.kind = ExprKind::Block(lets);
        }
    }

    fn arg(&mut self, a: &mut Arg) {
        match a {
            Arg::Positional(x) | Arg::Keyword { value: x, .. } => self.expr(x),
            Arg::Placeholder(_) => {}
        }
    }

    /// Recurse into the sub-expressions of `e`.
    /// EXTEND: every new `ExprKind` with children needs an arm.
    fn children(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit
            | ExprKind::Name(_) => {}
            ExprKind::Not(x) | ExprKind::Assert(x) | ExprKind::Unary { operand: x, .. } => {
                self.expr(x)
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Logic { lhs, rhs, .. }
            | ExprKind::Pipe { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                for link in rest {
                    self.expr(&mut link.rhs);
                }
            }
            ExprKind::If {
                cond,
                then_br,
                else_br,
            } => {
                self.expr(cond);
                self.expr(then_br);
                self.expr(else_br);
            }
            ExprKind::Block(items) => {
                for i in items {
                    self.item(i);
                }
            }
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee);
                for a in args {
                    self.arg(a);
                }
            }
            ExprKind::Lambda(l) => self.expr(&mut l.body),
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

    #[test]
    fn partials_become_lambdas() {
        assert_eq!(
            desugared("f(_, 2)"),
            "(expr (block (let $a0 2) (expr (lambda partial:f (($p0)) (call f $p0 $a0)))))\n"
        );
        assert_eq!(
            desugared("f(_, _)"),
            "(expr (lambda partial:f (($p0) ($p1)) (call f $p0 $p1)))\n"
        );
        assert_eq!(
            desugared("g(1)(_, k=2)"),
            "(expr (block (let $c (call g 1)) (let $a0 2) \
             (expr (lambda partial:<expr> (($p0)) (call $c $p0 k=$a0)))))\n"
        );
        // A partial inside an argument is rewritten first.
        assert_eq!(
            desugared("h(f(_, 1))"),
            "(expr (call h (block (let $a0 1) (expr (lambda partial:f (($p0)) (call f $p0 $a0))))))\n"
        );
    }

    #[test]
    fn pipes_fill_placeholders() {
        assert_eq!(desugared("x |> f(1, _)"), "(expr (pcall f 1 x))\n");
        // A second `_`: a partial with the piped value fixed, evaluated first.
        assert_eq!(
            desugared("x |> f(a, _, _)"),
            "(expr (block (let $a1 x) (let $a0 a) \
             (expr (lambda partial:f (($p0)) (call f $a0 $a1 $p0)))))\n"
        );
    }
}
