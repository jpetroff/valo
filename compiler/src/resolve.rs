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
//! ## Rules (plan D4, D14, syntax.md S5, S13, S19)
//!
//! - **Global slots** are numbered before anything is resolved: first the
//!   built-ins (`vm::builtins::NATIVES`, slots `0..N`, the *prelude*), then
//!   every top-level `fn` and every top-level `let` in source order.
//! - **Top-level `fn`s are hoisted** (Rust item rule): visible in the whole
//!   file. A top-level `let` with the same name shadows the fn from its line
//!   on. Two top-level fns with the same name: E0207.
//! - `let` inside a block or fn body is a **local**, visible until the end
//!   of its block. Parameters are locals of their fn.
//! - Shadowing is allowed in the same scope (Q7=b): every `let` makes a new
//!   binding. The initialiser is resolved *before* the new name is added, so
//!   `let x = x + 1` reads the previous `x`.
//! - Lookup order: locals (innermost block first, newest first), then
//!   top-level `let`s seen so far (newest first), then top-level fns, then
//!   the prelude.
//! - **Inside a fn body** "top-level `let`s seen so far" means: those before
//!   the `fn`. If the name is still not found, the first *later* top-level
//!   `let` of that name is used (a forward reference; reading it before it
//!   is set is the runtime error R0302).
//! - **Signatures**: top-level fns and natives have a known signature, and so
//!   do bindings initialised with one of them (`let g = f`). Calls to a known
//!   signature are checked here (E0205) instead of at runtime (R0207).
//! - **Types** in annotations live in their own namespace: `int float bool
//!   str none any error`, `list[T]`, `dict[K, V]`, `tuple[…]` and the fn's
//!   type variables (E0402 unknown name, E0403 wrong number of type
//!   arguments). They are only checked for existence here; values are
//!   checked at runtime from M10.
//! - Errors: E0201 undefined name (+ did-you-mean), E0203 top-level name used
//!   before its definition, E0205 wrong number of arguments, E0206 `_` used
//!   as a value, E0207 defined twice, E0402/E0403 bad type, E0009 construct
//!   of a later milestone (nested `fn`, `_` placeholder, keyword arguments).
//!
//! ## Where to change things
//!
//! | I want to…                                | Look at                                     |
//! |-------------------------------------------|---------------------------------------------|
//! | resolve a new expression kind             | `Resolver::expr` (recurse into children)     |
//! | add a scope-introducing construct         | push/pop `Resolver::locals` around it (see `ExprKind::Block`, `fn_def`) |
//! | change the lookup order                   | `Resolver::lookup`                           |
//! | change undefined-name messages            | `Resolver::undefined`, `suggest`             |
//! | change argument checks of calls           | `Resolver::call`                             |
//! | add a builtin type name                   | `SCALAR_TYPES`, `GENERIC_TYPES`, `Resolver::check_type` |
//! | change the `--dump resolved` format       | `dump`                                       |
//!
//! EXTEND: M4b normalises keyword arguments in `call`; M6 adds capture
//! analysis for nested fns and lambdas.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `HashMap<NodeId, Res>` — side tables keyed by node id (*Structs*).
//! - `.iter().rev().find_map(..)` — search from the end (*Closures and iterators*).
//! - newtypes `BindingId(u32)`, `GlobalId(u32)` so the two kinds of index
//!   cannot be mixed up (*Structs*).
//! - `std::mem::take` to save and restore a field around a nested walk (*Ownership and borrowing*).

use std::collections::HashMap;
use std::fmt::Write as _;

use valo_vm::builtins::NATIVES;

use crate::ast::{Arg, Binding, Expr, ExprKind, FnDef, Item, NodeId, Program, TypeExpr, TypeKind};
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

/// A declared local, for diagnostics and dumps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefInfo {
    /// Source name.
    pub name: String,
    /// Span of the name where it is defined.
    pub span: Span,
}

/// What a global slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalSlot {
    /// Built-in `NATIVES[i]`.
    Native(u16),
    /// A top-level `fn`.
    Fn,
    /// A top-level `let`.
    Let,
}

/// A global slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalInfo {
    /// Source name.
    pub name: String,
    /// Span of the defining name (empty for built-ins).
    pub span: Span,
    /// Kind of slot.
    pub slot: GlobalSlot,
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
    /// Defining node → the slot it defines: `Binding` (`None` for `let _`),
    /// `Param` (a local) and `FnDef` (a global).
    pub lets: HashMap<NodeId, Option<Res>>,
    /// Global slots, indexed by `GlobalId` (built-ins first).
    pub globals: Vec<GlobalInfo>,
    /// Local bindings, indexed by `BindingId`.
    pub locals: Vec<DefInfo>,
    /// Every name use, in source order.
    pub uses: Vec<Use>,
}

/// Scalar type names (S4, S9, S10, S13).
const SCALAR_TYPES: &[&str] = &["int", "float", "bool", "str", "none", "any", "error"];
/// Generic type names and their number of type arguments (`None` = one or more).
const GENERIC_TYPES: &[(&str, Option<usize>)] =
    &[("list", Some(1)), ("dict", Some(2)), ("tuple", None)];

/// Parameters of a statically known callee.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sig {
    /// Display name.
    name: String,
    /// Parameter names in order.
    params: Vec<String>,
    /// Where it is defined (`None` for built-ins).
    def: Option<Span>,
}

/// Resolve every name in `program`.
pub fn resolve(program: &Program) -> Result<Resolved, Vec<Diagnostic>> {
    let mut r = Resolver {
        prelude: Vec::new(),
        items: Vec::new(),
        all_lets: Vec::new(),
        lets_done: 0,
        locals: Vec::new(),
        fn_ctx: None,
        sigs: Vec::new(),
        known: HashMap::new(),
        out: Resolved::default(),
        diags: Vec::new(),
    };
    r.declare_globals(program);
    for item in &program.items {
        r.item(item, true);
    }
    if r.diags.is_empty() {
        Ok(r.out)
    } else {
        Err(r.diags)
    }
}

/// While resolving a fn body: what the fn may see.
#[derive(Debug, Clone)]
struct FnCtx {
    /// Number of top-level `let`s before the fn (`all_lets[..top_len]`).
    top_len: usize,
    /// Declared type variables.
    type_params: Vec<String>,
}

struct Resolver {
    /// Built-ins (global slots `0..N`).
    prelude: Vec<(String, GlobalId)>,
    /// Top-level fns (hoisted: visible everywhere).
    items: Vec<(String, GlobalId)>,
    /// Every top-level `let` (not `_`) in source order.
    all_lets: Vec<(String, Span, GlobalId)>,
    /// How many of `all_lets` the top-level walk has passed.
    lets_done: usize,
    /// Local scopes of the code being resolved (block scopes at top level,
    /// the fn's own scopes inside a fn body); innermost last.
    locals: Vec<Vec<(String, BindingId)>>,
    /// Set while resolving a fn body.
    fn_ctx: Option<FnCtx>,
    /// Known signatures.
    sigs: Vec<Sig>,
    /// Name target → index into `sigs`.
    known: HashMap<Res, usize>,
    out: Resolved,
    diags: Vec<Diagnostic>,
}

impl Resolver {
    /// Pre-pass: number every global slot (built-ins, top-level fns and
    /// lets) and record the signatures of fns and built-ins.
    fn declare_globals(&mut self, program: &Program) {
        for (i, n) in NATIVES.iter().enumerate() {
            let g = self.new_global(n.name, Span::default(), GlobalSlot::Native(i as u16));
            self.prelude.push((n.name.to_string(), g));
            self.add_sig(
                Res::Global(g),
                Sig {
                    name: n.name.to_string(),
                    params: n.params.iter().map(|p| p.name.to_string()).collect(),
                    def: None,
                },
            );
        }
        for item in &program.items {
            match item {
                Item::Let(b) if b.name != "_" => {
                    let g = self.new_global(&b.name, b.name_span, GlobalSlot::Let);
                    self.all_lets.push((b.name.clone(), b.name_span, g));
                }
                Item::Fn(d) => self.declare_fn(d),
                _ => {}
            }
        }
    }

    fn declare_fn(&mut self, d: &FnDef) {
        if let Some((_, first)) = self.items.iter().find(|(n, _)| *n == d.name) {
            let first_span = self.out.globals[first.0 as usize].span;
            self.diags.push(
                Diagnostic::new(
                    "E0207",
                    format!("fn `{}` is defined twice", d.name),
                    d.name_span,
                )
                .label("second definition")
                .with(first_span, "first defined here")
                .help("top-level fns are visible in the whole file, so each name can be defined only once; rename one of them"),
            );
            return;
        }
        let g = self.new_global(&d.name, d.name_span, GlobalSlot::Fn);
        self.out.lets.insert(d.id, Some(Res::Global(g)));
        self.items.push((d.name.clone(), g));
        self.add_sig(
            Res::Global(g),
            Sig {
                name: d.name.clone(),
                params: d.params.iter().map(|p| p.name.clone()).collect(),
                def: Some(d.name_span),
            },
        );
    }

    fn new_global(&mut self, name: &str, span: Span, slot: GlobalSlot) -> GlobalId {
        self.out.globals.push(GlobalInfo {
            name: name.to_string(),
            span,
            slot,
        });
        GlobalId(self.out.globals.len() as u32 - 1)
    }

    fn add_sig(&mut self, res: Res, sig: Sig) {
        self.sigs.push(sig);
        self.known.insert(res, self.sigs.len() - 1);
    }

    fn new_local(&mut self, name: &str, span: Span) -> BindingId {
        self.out.locals.push(DefInfo {
            name: name.to_string(),
            span,
        });
        BindingId(self.out.locals.len() as u32 - 1)
    }

    /// `top_level`: the item is a line of the file itself, not of a block.
    fn item(&mut self, item: &Item, top_level: bool) {
        match item {
            Item::Expr(e) => self.expr(e),
            Item::Let(b) => self.binding(b, top_level),
            Item::Fn(d) if top_level => self.fn_def(d),
            Item::Fn(d) => self.diags.push(
                Diagnostic::new(
                    "E0009",
                    "nested functions are not supported yet",
                    d.name_span,
                )
                .label("not implemented in this version of valo")
                .help("define the fn at the top level of the file"),
            ),
        }
    }

    /// `let`: annotation and initialiser first (the initialiser sees the
    /// *previous* binding of the same name), then the new binding.
    fn binding(&mut self, b: &Binding, top_level: bool) {
        if let Some(t) = &b.ann {
            self.check_type(t);
        }
        self.expr(&b.init);
        // `let g = f` gives `g` the signature of `f`.
        let sig = match &b.init.kind {
            ExprKind::Name(_) => self
                .out
                .names
                .get(&b.init.id)
                .and_then(|r| self.known.get(r))
                .copied(),
            _ => None,
        };
        if b.name == "_" {
            self.out.lets.insert(b.id, None);
            return;
        }
        let res = if top_level {
            let (_, _, g) = self.all_lets[self.lets_done];
            self.lets_done += 1;
            Res::Global(g)
        } else {
            let id = self.new_local(&b.name, b.name_span);
            self.locals
                .last_mut()
                .expect("a block has a scope")
                .push((b.name.clone(), id));
            Res::Local(id)
        };
        self.out.lets.insert(b.id, Some(res));
        if let Some(i) = sig {
            self.known.insert(res, i);
        }
    }

    /// A top-level fn: parameters become locals of a fresh scope stack; the
    /// body sees the top-level `let`s defined before the fn (plus later
    /// ones as forward references, see `lookup`).
    fn fn_def(&mut self, d: &FnDef) {
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_ctx = self.fn_ctx.replace(FnCtx {
            top_len: self.lets_done,
            type_params: d.type_params.iter().map(|(n, _)| n.clone()).collect(),
        });
        for (i, (name, span)) in d.type_params.iter().enumerate() {
            if let Some((_, first)) = d.type_params[..i].iter().find(|(n, _)| n == name) {
                self.diags.push(
                    Diagnostic::new(
                        "E0207",
                        format!("type variable `{name}` is declared twice"),
                        *span,
                    )
                    .label("second declaration")
                    .with(*first, "first declared here"),
                );
            }
        }
        let mut scope: Vec<(String, BindingId)> = Vec::new();
        for p in &d.params {
            if let Some(t) = &p.ann {
                self.check_type(t);
            }
            if let Some((_, first)) = scope.iter().find(|(n, _)| *n == p.name) {
                let first_span = self.out.locals[first.0 as usize].span;
                self.diags.push(
                    Diagnostic::new(
                        "E0207",
                        format!("parameter `{}` is defined twice in fn `{}`", p.name, d.name),
                        p.span,
                    )
                    .label("second definition")
                    .with(first_span, "first defined here"),
                );
            }
            let id = self.new_local(&p.name, p.span);
            self.out.lets.insert(p.id, Some(Res::Local(id)));
            scope.push((p.name.clone(), id));
        }
        if let Some(t) = &d.ret {
            self.check_type(t);
        }
        self.locals.push(scope);
        self.expr(&d.body);
        self.locals = saved_locals;
        self.fn_ctx = saved_ctx;
    }

    /// EXTEND: every new `ExprKind` with children recurses here.
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Unit => {}
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
                self.locals.push(Vec::new());
                for item in items {
                    self.item(item, false);
                }
                self.locals.pop();
            }
            ExprKind::Call { callee, args, .. } => self.call(e, callee, args),
            ExprKind::Pipe { .. } => unreachable!("pipes are removed by desugar.rs"),
        }
    }

    /// A call: resolve callee and arguments, then check the argument count
    /// against a known signature (E0205).
    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Arg]) {
        self.expr(callee);
        let mut positional = 0;
        for a in args {
            match a {
                Arg::Positional(x) => {
                    positional += 1;
                    self.expr(x);
                }
                Arg::Keyword {
                    value, name_span, ..
                } => {
                    self.expr(value);
                    self.diags.push(
                        Diagnostic::new(
                            "E0009",
                            "keyword arguments are not supported yet",
                            *name_span,
                        )
                        .label("not implemented in this version of valo"),
                    );
                    return;
                }
                Arg::Placeholder(span) => {
                    self.diags.push(
                        Diagnostic::new(
                            "E0009",
                            "partial application with `_` is not supported yet",
                            *span,
                        )
                        .label("not implemented in this version of valo"),
                    );
                    return;
                }
            }
        }
        let ExprKind::Name(_) = callee.kind else {
            return;
        };
        let Some(&i) = self
            .out
            .names
            .get(&callee.id)
            .and_then(|r| self.known.get(r))
        else {
            return;
        };
        let sig = &self.sigs[i];
        let want = sig.params.len();
        if positional == want {
            return;
        }
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut d = Diagnostic::new(
            "E0205",
            format!(
                "`{}` takes {want} argument{} but {positional} {} given",
                sig.name,
                plural(want),
                if positional == 1 { "was" } else { "were" }
            ),
            e.span,
        )
        .label(format!(
            "called with {positional} argument{}",
            plural(positional)
        ));
        if let Some(def) = sig.def {
            d = d.with(def, "defined here");
        }
        if positional < want {
            let shown: Vec<&str> = sig
                .params
                .iter()
                .enumerate()
                .map(|(j, p)| if j < positional { p.as_str() } else { "_" })
                .collect();
            d = d.help(format!(
                "to fix only some arguments, use a partial: `{}({})`",
                sig.name,
                shown.join(", ")
            ));
        }
        self.diags.push(d);
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
        match self.lookup(name) {
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

    /// Find what `name` refers to here (order: see the module docs).
    fn lookup(&self, name: &str) -> Option<Res> {
        // RUST: `find_map` returns the first `Some` produced by the closure.
        let local = self
            .locals
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|(n, _)| n == name).map(|(_, b)| *b));
        if let Some(b) = local {
            return Some(Res::Local(b));
        }
        let visible = self.fn_ctx.as_ref().map_or(self.lets_done, |c| c.top_len);
        let global = self.all_lets[..visible]
            .iter()
            .rev()
            .find(|(n, _, _)| n == name)
            .map(|(_, _, g)| *g)
            .or_else(|| self.items.iter().find(|(n, _)| n == name).map(|(_, g)| *g))
            .or_else(|| {
                self.prelude
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, g)| *g)
            });
        if let Some(g) = global {
            return Some(Res::Global(g));
        }
        // Forward reference from a fn body to a later top-level `let`.
        if self.fn_ctx.is_some() {
            return self.all_lets[visible..]
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, _, g)| Res::Global(*g));
        }
        None
    }

    /// E0203 if a top-level `let` of this name comes later, else E0201.
    fn undefined(&mut self, span: Span, name: &str) {
        if let Some((_, def, _)) = self.all_lets[self.lets_done..]
            .iter()
            .find(|(n, _, _)| n == name)
        {
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
        let visible_lets = self.fn_ctx.as_ref().map_or(self.lets_done, |c| c.top_len);
        let visible: Vec<&str> = self
            .prelude
            .iter()
            .chain(&self.items)
            .map(|(n, _)| n.as_str())
            .chain(
                self.all_lets[..visible_lets]
                    .iter()
                    .map(|(n, _, _)| n.as_str()),
            )
            .chain(self.locals.iter().flatten().map(|(n, _)| n.as_str()))
            .collect();
        let mut d = Diagnostic::new("E0201", format!("undefined name `{name}`"), span)
            .label("not found in this scope");
        if let Some(s) = suggest(name, &visible) {
            d = d.help(format!("did you mean `{s}`?"));
        }
        self.diags.push(d);
    }

    /// Check that every name in an annotation is a type (E0402) used with
    /// the right number of type arguments (E0403).
    fn check_type(&mut self, t: &TypeExpr) {
        match &t.kind {
            TypeKind::Unit => {}
            TypeKind::Name(n) => {
                if SCALAR_TYPES.contains(&n.as_str()) || self.is_type_var(n) {
                    return;
                }
                if let Some((_, count)) = GENERIC_TYPES.iter().find(|(g, _)| g == n) {
                    let example = match count {
                        Some(2) => "dict[str, int]",
                        Some(_) => "list[int]",
                        None => "tuple[int, str]",
                    };
                    self.diags.push(
                        Diagnostic::new(
                            "E0403",
                            format!("`{n}` needs type arguments, e.g. {example}"),
                            t.span,
                        )
                        .label("missing `[…]`"),
                    );
                    return;
                }
                self.unknown_type(t.span, n);
            }
            TypeKind::Generic(n, args) => {
                match GENERIC_TYPES.iter().find(|(g, _)| g == n) {
                    Some((_, Some(want))) if *want != args.len() => {
                        self.diags.push(
                            Diagnostic::new(
                                "E0403",
                                format!(
                                    "`{n}` takes {want} type argument{}, found {}",
                                    if *want == 1 { "" } else { "s" },
                                    args.len()
                                ),
                                t.span,
                            )
                            .label("wrong number of type arguments"),
                        );
                    }
                    Some(_) => {}
                    None if SCALAR_TYPES.contains(&n.as_str()) || self.is_type_var(n) => {
                        self.diags.push(
                            Diagnostic::new(
                                "E0403",
                                format!("`{n}` takes no type arguments"),
                                t.span,
                            )
                            .label("remove the `[…]`"),
                        );
                    }
                    None => self.unknown_type(t.span, n),
                }
                for a in args {
                    self.check_type(a);
                }
            }
            TypeKind::Tuple(ts) => {
                for x in ts {
                    self.check_type(x);
                }
            }
            TypeKind::Union(a, b) | TypeKind::Result(a, b) => {
                self.check_type(a);
                self.check_type(b);
            }
            TypeKind::Fn(params, ret) => {
                for x in params {
                    self.check_type(x);
                }
                self.check_type(ret);
            }
        }
    }

    fn is_type_var(&self, n: &str) -> bool {
        self.fn_ctx
            .as_ref()
            .is_some_and(|c| c.type_params.iter().any(|t| t == n))
    }

    fn unknown_type(&mut self, span: Span, name: &str) {
        let mut candidates: Vec<&str> = SCALAR_TYPES
            .iter()
            .copied()
            .chain(GENERIC_TYPES.iter().map(|(g, _)| *g))
            .collect();
        if let Some(c) = &self.fn_ctx {
            candidates.extend(c.type_params.iter().map(String::as_str));
        }
        let mut d =
            Diagnostic::new("E0402", format!("unknown type `{name}`"), span).label("not a type");
        let capitalised = name.starts_with(|c: char| c.is_uppercase());
        match suggest(name, &candidates) {
            Some(s) => d = d.help(format!("did you mean `{s}`?")),
            None if capitalised && self.fn_ctx.is_some() => {
                d = d.help(format!(
                    "a type variable must be declared after the fn name: `fn f[{name}](…)`"
                ));
            }
            None => {
                d = d.help(
                    "the types are int, float, bool, str, none, any, error, list[T], dict[K, V], tuple[…]",
                );
            }
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

/// Text form for `--dump resolved`: built-ins, declared globals and locals,
/// then every name use with its target.
pub fn dump(r: &Resolved, sm: &SourceMap) -> String {
    let mut out = String::new();
    let natives: Vec<&str> = r
        .globals
        .iter()
        .filter(|g| matches!(g.slot, GlobalSlot::Native(_)))
        .map(|g| g.name.as_str())
        .collect();
    let _ = writeln!(
        out,
        "natives: global#0..#{} ({})",
        natives.len().saturating_sub(1),
        natives.join(" ")
    );
    let user: Vec<(usize, &GlobalInfo)> = r
        .globals
        .iter()
        .enumerate()
        .filter(|(_, g)| !matches!(g.slot, GlobalSlot::Native(_)))
        .collect();
    if user.is_empty() {
        out.push_str("globals: (none)\n");
    } else {
        out.push_str("globals:\n");
        for (i, g) in user {
            let (l, c) = sm.line_col(g.span.start);
            let pos = format!("{l}:{c}");
            if g.slot == GlobalSlot::Fn {
                let _ = writeln!(out, "  global#{i:<3} {:<12} {pos:<7} fn", g.name);
            } else {
                let _ = writeln!(out, "  global#{i:<3} {:<12} {pos}", g.name);
            }
        }
    }
    if r.locals.is_empty() {
        out.push_str("locals: (none)\n");
    } else {
        out.push_str("locals:\n");
        for (i, d) in r.locals.iter().enumerate() {
            let (l, c) = sm.line_col(d.span.start);
            let _ = writeln!(out, "  local#{i:<3} {:<12} {l}:{c}", d.name);
        }
    }
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
        let p = crate::desugar::desugar(parse_source(&sm).expect("parses"));
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

    // Snapshot: src/snapshots/valo_compiler__resolve__tests__functions_and_globals.snap
    #[test]
    fn functions_and_globals() {
        // `twice` is used before its definition (hoisting); `limit` inside
        // `scaled` is a forward reference to a later `let`; `inc` is
        // shadowed by a `let` from line 6 on, and `inc2` keeps the fn's
        // signature.
        insta::assert_snapshot!(
            resolve_src(
                "let a = twice(2)\nfn twice(x: int) -> int = x * 2\n\
                 fn scaled(x: int) -> int = x * limit\nlet limit = 10\n\
                 fn inc(x: int) -> int = x + 1\nlet inc2 = inc\nlet inc = 5\nprint(inc2(inc))"
            )
            .unwrap()
        );
    }

    #[test]
    fn fn_bodies_see_globals_as_at_definition() {
        let src = "let x = 1\nfn f() -> int = x\nlet x = 2\nf()";
        let d = resolve_src(src).unwrap();
        assert!(d.contains("2:17    x -> global#3"), "{d}");
    }

    #[test]
    fn function_errors() {
        let codes = |s| resolve_src(s).unwrap_err();
        assert_eq!(codes("fn f() -> int = 1\nfn f() -> int = 2"), ["E0207"]);
        assert_eq!(codes("fn f(x: int, x: int) -> int = x"), ["E0207"]);
        assert_eq!(codes("fn f[T, T](x: T) -> T = x"), ["E0207"]);
        assert_eq!(codes("fn f(x: int) -> int = x\nf()"), ["E0205"]);
        assert_eq!(
            codes("fn f(x: int) -> int = x\nlet g = f\ng(1, 2)"),
            ["E0205"]
        );
        assert_eq!(codes("print()"), ["E0205"]);
        assert_eq!(codes("fn f(x: itn) -> int = x"), ["E0402"]);
        assert_eq!(codes("fn f(x: T) -> int = 1"), ["E0402"]);
        assert_eq!(
            codes("fn f[T](x: list[T]) -> T | none = none_value"),
            ["E0201"]
        );
        assert_eq!(codes("let x: list = 1"), ["E0403"]);
        assert_eq!(codes("let x: int[str] = 1"), ["E0403"]);
        assert_eq!(codes("let x: dict[str] = 1"), ["E0403"]);
        assert_eq!(
            codes("let v = if true:\n    fn g() -> int = 1\n    g()\nelse: 0"),
            ["E0009", "E0201"]
        );
        assert_eq!(codes("fn f(x: int) -> int = x\nf(_)"), ["E0009"]);
        assert_eq!(codes("fn f(x: int) -> int = x\nf(x=1)"), ["E0009"]);
        // In main, a later `let` is still E0203; from a fn body it is fine.
        assert_eq!(codes("print(y)\nlet y = 1"), ["E0203"]);
        assert!(resolve_src("fn f() -> int = y\nlet y = 1").is_ok());
        // Fine: dynamic callee, and correct counts.
        assert!(
            resolve_src("fn f(x: int) -> int = x\nlet h = if true then f else f\nh(1, 2)").is_ok()
        );
        assert!(
            resolve_src("fn f[T](x: T, d: dict[str, list[T]]) -> (T, any) ! error = x").is_ok()
        );
    }
}
