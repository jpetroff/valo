//! # Name resolution (`resolve.rs`)
//!
//! **What this file is:** the pass between parsing and code generation that
//! answers "which binding does this name refer to?". For every `Name`
//! expression it records a `Res` (a global slot or a local binding), and for
//! every `let` it records which slot the binding defines. Code generation
//! reads these side tables (keyed by `ast::NodeId`) instead of looking names
//! up itself.
//!
//! **Pipeline position:** `ast::Program` in → `Resolved` out → `codegen.rs`.
//! Entry point: `resolve`. Printed by `valo check --dump resolved` (`dump`).
//!
//! **Textbook chapter:** `docs/11-roadmap.md` (M2) — a chapter of its own
//! follows once functions and closures (M4, M6) make it interesting.
//!
//! ## Rules (plan D4, syntax.md S5)
//!
//! - Top-level `let` bindings are **globals** (one slot each, indexed at
//!   compile time); `let` inside a block (an indented `if` branch, later a
//!   `fn` body) is a **local**, visible until the end of its block.
//! - Shadowing is allowed in the same scope (Q7=b): every `let` makes a new
//!   binding. The initialiser is resolved *before* the new name is added, so
//!   `let x = x + 1` reads the previous `x`.
//! - Scopes are a stack; lookup searches innermost first and, inside a
//!   scope, newest first.
//! - Errors: E0201 undefined name (+ did-you-mean), E0203 top-level name used
//!   before its definition, E0206 `_` used as a value.
//!
//! ## Where to change things
//!
//! | I want to…                                | Look at                                     |
//! |-------------------------------------------|---------------------------------------------|
//! | resolve a new expression kind             | `Resolver::expr` (recurse into children)     |
//! | add a scope-introducing construct         | `push_scope` / `pop_scope` around it         |
//! | change undefined-name messages            | `Resolver::undefined`, `suggest`             |
//! | change the `--dump resolved` format       | `dump`                                       |
//!
//! EXTEND: M4 adds functions (params as locals, hoisted top-level fns,
//! natives seeded as globals 0..N, arity checks); M6 adds capture analysis.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `HashMap<NodeId, Res>` — side tables keyed by node id (*Structs*).
//! - `.iter().rev().find_map(..)` — search from the end (*Closures and iterators*).
//! - newtypes `BindingId(u32)`, `GlobalId(u32)` so the two kinds of index
//!   cannot be mixed up (*Structs*).

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::ast::{Binding, Expr, ExprKind, Item, NodeId, Program};
use crate::diagnostic::Diagnostic;
use crate::span::{SourceMap, Span};

/// Index of a local binding (unique per program; codegen maps it to a register).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingId(pub u32);

/// Index of a global slot (`GETGLOBAL`/`SETGLOBAL` operand).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalId(pub u32);

/// What a name refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Res {
    /// A local binding (lives in a register).
    Local(BindingId),
    /// A global slot.
    Global(GlobalId),
}

/// A declared global or local, for diagnostics and dumps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefInfo {
    /// Source name.
    pub name: String,
    /// Span of the name in its `let`.
    pub span: Span,
}

/// One resolved name use, in source order (for `--dump resolved`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    /// Span of the name.
    pub span: Span,
    /// The name.
    pub name: String,
    /// Its target.
    pub res: Res,
}

/// Output of name resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    /// `ExprKind::Name` node → its target.
    pub names: HashMap<NodeId, Res>,
    /// `Binding` node → the slot it defines (`None` for `let _`).
    pub lets: HashMap<NodeId, Option<Res>>,
    /// Global slots, indexed by `GlobalId`.
    pub globals: Vec<DefInfo>,
    /// Local bindings, indexed by `BindingId`.
    pub locals: Vec<DefInfo>,
    /// Every name use, in source order.
    pub uses: Vec<Use>,
}

/// Resolve every name in `program`.
pub fn resolve(program: &Program) -> Result<Resolved, Vec<Diagnostic>> {
    let mut r = Resolver {
        scopes: vec![Vec::new()],
        toplevel: program
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Let(b) if b.name != "_" => Some((b.name.clone(), b.name_span)),
                _ => None,
            })
            .collect(),
        out: Resolved::default(),
        diags: Vec::new(),
    };
    for item in &program.items {
        r.item(item);
    }
    if r.diags.is_empty() {
        Ok(r.out)
    } else {
        Err(r.diags)
    }
}

struct Resolver {
    /// Scope stack; `scopes[0]` is the top level (globals).
    scopes: Vec<Vec<(String, Res)>>,
    /// Every top-level `let` name, for E0203.
    toplevel: Vec<(String, Span)>,
    out: Resolved,
    diags: Vec<Diagnostic>,
}

impl Resolver {
    fn item(&mut self, item: &Item) {
        match item {
            Item::Expr(e) => self.expr(e),
            Item::Let(b) => self.binding(b),
        }
    }

    /// `let`: initialiser first (it sees the *previous* binding of the
    /// same name), then the new binding.
    fn binding(&mut self, b: &Binding) {
        self.expr(&b.init);
        if b.name == "_" {
            self.out.lets.insert(b.id, None);
            return;
        }
        let info = DefInfo {
            name: b.name.clone(),
            span: b.name_span,
        };
        let res = if self.scopes.len() == 1 {
            self.out.globals.push(info);
            Res::Global(GlobalId(self.out.globals.len() as u32 - 1))
        } else {
            self.out.locals.push(info);
            Res::Local(BindingId(self.out.locals.len() as u32 - 1))
        };
        self.out.lets.insert(b.id, Some(res));
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .push((b.name.clone(), res));
    }

    /// EXTEND: every new `ExprKind` with children recurses here.
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => {}
            ExprKind::Name(name) => self.name(e, name),
            ExprKind::Unary { operand, .. }
            | ExprKind::Not(operand)
            | ExprKind::Assert(operand) => self.expr(operand),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Logic { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                for link in rest {
                    self.expr(&link.rhs);
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
                self.scopes.push(Vec::new());
                for item in items {
                    self.item(item);
                }
                self.scopes.pop();
            }
        }
    }

    fn name(&mut self, e: &Expr, name: &str) {
        if name == "_" {
            self.diags.push(
                Diagnostic::new("E0206", "`_` is not a value", e.span)
                    .label("`_` cannot be read")
                    .help("`_` discards a value (`let _ = …`); it is not a variable"),
            );
            return;
        }
        // RUST: `find_map` returns the first `Some` produced by the closure.
        let found = self
            .scopes
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|(n, _)| n == name).map(|(_, r)| *r));
        match found {
            Some(res) => {
                self.out.names.insert(e.id, res);
                self.out.uses.push(Use {
                    span: e.span,
                    name: name.to_string(),
                    res,
                });
            }
            None => self.undefined(e.span, name),
        }
    }

    /// E0203 if a top-level `let` of this name comes later, else E0201.
    fn undefined(&mut self, span: Span, name: &str) {
        if let Some((_, def)) = self.toplevel.iter().find(|(n, _)| n == name) {
            self.diags.push(
                Diagnostic::new(
                    "E0203",
                    format!("`{name}` is used before its definition"),
                    span,
                )
                .label("used here")
                .with(*def, "defined here")
                .help("top-level code runs from top to bottom; move the definition above this use"),
            );
            return;
        }
        let visible: Vec<&str> = self
            .scopes
            .iter()
            .flat_map(|s| s.iter().map(|(n, _)| n.as_str()))
            .collect();
        let mut d = Diagnostic::new("E0201", format!("undefined name `{name}`"), span)
            .label("not found in this scope");
        if let Some(s) = suggest(name, &visible) {
            d = d.help(format!("did you mean `{s}`?"));
        }
        self.diags.push(d);
    }
}

/// The candidate closest to `name` by edit distance, if close enough
/// (at most `max(1, len / 3)` edits). Used for "did you mean" hints.
pub fn suggest<'a>(name: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .iter()
        .rev()
        .map(|c| (edit_distance(name, c), *c))
        .filter(|(d, _)| *d <= limit)
        // Smallest distance; ties go to the most recently defined name.
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Edit distance counting insert, delete, substitute and swapping two
/// neighbouring characters as one edit each ("optimal string alignment"), so
/// the typo `cuont` is one edit away from `count`.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // d[i][j] = distance between the first i chars of a and first j of b.
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[a.len()][b.len()]
}

fn res_str(r: Res) -> String {
    match r {
        Res::Local(BindingId(i)) => format!("local#{i}"),
        Res::Global(GlobalId(i)) => format!("global#{i}"),
    }
}

/// Text form for `--dump resolved`: declared globals and locals, then every
/// name use with its target.
pub fn dump(r: &Resolved, sm: &SourceMap) -> String {
    let mut out = String::new();
    let table = |out: &mut String, title: &str, prefix: &str, defs: &[DefInfo]| {
        if defs.is_empty() {
            let _ = writeln!(out, "{title}: (none)");
            return;
        }
        let _ = writeln!(out, "{title}:");
        for (i, d) in defs.iter().enumerate() {
            let (l, c) = sm.line_col(d.span.start);
            let _ = writeln!(out, "  {prefix}#{i:<3} {:<12} {l}:{c}", d.name);
        }
    };
    table(&mut out, "globals", "global", &r.globals);
    table(&mut out, "locals", "local", &r.locals);
    if r.uses.is_empty() {
        out.push_str("uses: (none)\n");
    } else {
        out.push_str("uses:\n");
        for u in &r.uses {
            let (l, c) = sm.line_col(u.span.start);
            let pos = format!("{l}:{c}");
            let _ = writeln!(out, "  {pos:<7} {} -> {}", u.name, res_str(u.res));
        }
    }
    out
}

// Tests: `cargo test -p valo-compiler resolve`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_source;

    fn resolve_src(src: &str) -> Result<String, Vec<&'static str>> {
        let sm = SourceMap::new("t.vl", src);
        let p = parse_source(&sm).expect("parses");
        resolve(&p)
            .map(|r| dump(&r, &sm))
            .map_err(|ds| ds.iter().map(|d| d.code).collect())
    }

    // Snapshot: src/snapshots/valo_compiler__resolve__tests__toplevel_rebinding.snap
    #[test]
    fn toplevel_rebinding() {
        insta::assert_snapshot!(
            resolve_src("let x = 1\nlet x = x + 1\nlet y = x * x\ny + x").unwrap()
        );
    }

    // Snapshot: src/snapshots/valo_compiler__resolve__tests__block_shadowing.snap
    #[test]
    fn block_shadowing() {
        insta::assert_snapshot!(
            resolve_src(
                "let x = 1\nlet y = if true:\n    let x = x + 1\n    let x = x * 10\n    \
                 if x > 0:\n        let x = 0\n        x\n    else: x\nelse: x\nx + y"
            )
            .unwrap()
        );
    }

    #[test]
    fn block_locals_end_with_their_block() {
        assert_eq!(
            resolve_src("let a = if true:\n    let t = 1\n    t\nelse: 0\nt"),
            Err(vec!["E0201"])
        );
    }

    #[test]
    fn errors() {
        assert_eq!(resolve_src("y + 1\nlet y = 2"), Err(vec!["E0203"]));
        assert_eq!(resolve_src("let x = x"), Err(vec!["E0203"]));
        assert_eq!(resolve_src("let count = 1\ncuont"), Err(vec!["E0201"]));
        assert_eq!(resolve_src("let _ = 1\n_ + 1"), Err(vec!["E0206"]));
        assert!(resolve_src("let _ = 1\nlet _ = 2").is_ok());
    }

    #[test]
    fn suggestions() {
        assert_eq!(suggest("cuont", &["count", "x"]), Some("count"));
        assert_eq!(suggest("x", &["y"]), Some("y"));
        assert_eq!(suggest("total", &["x", "y"]), None);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("ab", "ba"), 1);
    }
}
