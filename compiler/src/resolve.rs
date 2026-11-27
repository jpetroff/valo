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
//! **Textbook chapter:** `docs/06-code-generation.md` (*Closures*, M6) and
//! `docs/11-roadmap.md`.
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
//!   of its block. Parameters are locals of their fn. A `fn` inside a block
//!   (S21) is a local too, visible from its line on; inside its own body its
//!   name is `Res::SelfRef` (the running closure, `GETSELF`).
//! - **Captures** (M6, plan D5): a lambda or nested fn that uses a local of
//!   an enclosing function captures it. Lookup walks the function stack
//!   outwards (Lua's `singlevar`); every function between the use and the
//!   definition gets a capture entry (`Resolved::captures`), and the use
//!   becomes `Res::Upvalue { index }`. Globals are never captured.
//! - A local fn may not use a *later* local fn of its block: local mutual
//!   recursion is E0204 (Q16); define such fns at the top level.
//! - Shadowing is allowed in the same scope (Q7=b): every `let` makes a new
//!   binding. The initialiser is resolved *before* the new name is added, so
//!   `let x = x + 1` reads the previous `x`.
//! - Lookup order: locals (innermost block first, newest first), then
//!   top-level `let`s seen so far (newest first), then top-level fns, then
//!   the prelude. Each function on the stack is searched before the
//!   globals.
//! - **Inside a fn body** "top-level `let`s seen so far" means: those before
//!   the `fn`. If the name is still not found, the first *later* top-level
//!   `let` of that name is used (a forward reference; reading it before it
//!   is set is the runtime error R0302).
//! - **Signatures**: top-level and local fns and natives have a known
//!   signature, and so do bindings initialised with one of them (`let g =
//!   f`), with a lambda or with a `_` partial (`let inc = add(1, _)`). Calls
//!   to a known signature are checked here (E0205) instead of at runtime
//!   (R0207).
//! - **Types** in annotations live in their own namespace: `int float bool
//!   str none any error`, `list[T]`, `dict[K, V]`, `tuple[…]` and the fn's
//!   type variables (E0402 unknown name, E0403 wrong number of type
//!   arguments). They are only checked for existence here; values are
//!   checked at runtime from M10.
//! - Errors: E0201 undefined name (+ did-you-mean), E0203 top-level name used
//!   before its definition, E0205 wrong number of arguments, E0206 `_` used
//!   as a value, E0207 defined twice, E0402/E0403 bad type, E0421 defaulted
//!   parameter passed by position, E0422 keyword arguments to an unknown
//!   callee, E0424 unknown keyword / required parameter passed by name,
//!   E0425 keyword given twice, E0204 local fn uses a later local fn, E0301
//!   more than 255 captures.
//! - **Keyword arguments** (S19, plan D28): for a call to a known signature,
//!   `plan_call` records which argument fills each parameter
//!   (`Resolved::calls`, a `CallPlan`); omitted defaulted parameters become
//!   `Slot::Missing`. Defaults are resolved in a scope holding only the
//!   earlier parameters.
//!
//! ## Where to change things
//!
//! | I want to…                                | Look at                                     |
//! |-------------------------------------------|---------------------------------------------|
//! | resolve a new expression kind             | `Resolver::expr` (recurse into children)     |
//! | add a scope-introducing construct         | push/pop a `Scope` on the current `FnScope` (see `ExprKind::Block`) |
//! | add a function-like construct             | push/pop a `FnScope` (see `fn_def`, `lambda`) |
//! | change the lookup order                   | `Resolver::lookup`, `lookup_in` (captures)    |
//! | change undefined-name messages            | `Resolver::undefined`, `suggest`             |
//! | change argument checks of calls           | `Resolver::call`, `plan_call`, `wrong_count` |
//! | add a builtin type name                   | `SCALAR_TYPES`, `GENERIC_TYPES`, `Resolver::check_type` |
//! | change the `--dump resolved` format       | `dump`                                       |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `HashMap<NodeId, Res>` — side tables keyed by node id (*Structs*).
//! - `.iter().rev().find_map(..)` — search from the end (*Closures and iterators*).
//! - newtypes `BindingId(u32)`, `GlobalId(u32)` so the two kinds of index
//!   cannot be mixed up (*Structs*).
//! - recursion over a stack of scopes with `&mut self` (`lookup_in`), adding
//!   capture entries on the way back out (*Ownership and borrowing*).

use std::collections::HashMap;
use std::fmt::Write as _;

use valo_vm::builtins::NATIVES;

use crate::ast::{
    Arg, Binding, Expr, ExprKind, FnDef, Item, Lambda, NodeId, Param, Program, StrPart, TypeExpr,
    TypeKind,
};
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
    /// A local binding of the current function (lives in a register).
    Local(BindingId),
    /// A global slot.
    Global(GlobalId),
    /// A local of an enclosing function, captured by the current one:
    /// `captures[index]` of the running closure (`GETUPVAL`, M6).
    Upvalue {
        /// The captured binding.
        binding: BindingId,
        /// Its position in the current function's capture list.
        index: u8,
    },
    /// The name of the local fn whose body this is: the running closure
    /// itself (`GETSELF`, M6). Never captured, so a closure never holds
    /// itself (no `Rc` cycle).
    SelfRef(BindingId),
}

impl Res {
    /// The local binding this refers to, if any.
    pub fn binding(self) -> Option<BindingId> {
        match self {
            Res::Local(b) | Res::Upvalue { binding: b, .. } | Res::SelfRef(b) => Some(b),
            Res::Global(_) => None,
        }
    }
}

/// Where a captured value comes from, seen from the enclosing function
/// that creates the closure (becomes `chunk::UpvalSrc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFrom {
    /// A local (register) of the enclosing function.
    Local(BindingId),
    /// A capture of the enclosing function itself.
    Upvalue(u8),
    /// The enclosing function's own closure (it is a local fn using its
    /// own name inside a lambda it creates).
    SelfFn,
}

/// One captured binding of a lambda or nested fn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// The binding (its name is `Resolved::locals[binding].name`).
    pub binding: BindingId,
    /// Where the enclosing function finds it.
    pub from: CaptureFrom,
}

/// The capture list of one lambda or nested fn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnCaptures {
    /// Fn name, or `lambda`.
    pub name: String,
    /// Where it is defined.
    pub span: Span,
    /// Index = upvalue index.
    pub list: Vec<Capture>,
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
    /// Calls to a statically known callee: which argument fills each
    /// parameter (M4b). Calls not listed pass their arguments as written.
    pub calls: HashMap<NodeId, CallPlan>,
    /// Captures of every lambda (keyed by its `Expr` id) and nested fn
    /// (keyed by its `FnDef` id), M6.
    pub captures: HashMap<NodeId, FnCaptures>,
}

/// How a call's arguments map onto the callee's parameters, in declaration
/// order (keyword arguments normalised, plan D28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPlan {
    /// One entry per parameter, up to the last one that is passed; omitted
    /// trailing defaulted parameters are left out (the VM fills them with
    /// `Missing`).
    pub slots: Vec<Slot>,
}

/// What fills one parameter slot of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// `args[i]` of the call (source order).
    Arg(usize),
    /// Nothing: the callee computes the default (`LOADMISSING`).
    Missing,
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
    /// Parameter names in order, and whether each has a default.
    params: Vec<(String, bool)>,
    /// How many leading parameters may be passed by position: the required
    /// ones, plus a built-in's omittable positional ones (`range(a, b)`,
    /// Q26). The rest are keyword-only (S19).
    positional: usize,
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
        funcs: vec![FnScope::default()],
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

/// One block scope.
#[derive(Debug, Default)]
struct Scope {
    /// Bindings in definition order.
    names: Vec<(String, BindingId)>,
    /// Local fns of this block not reached yet (for E0204).
    later_fns: Vec<(String, Span)>,
}

/// The scopes of one function being resolved (`<main>`, a fn, a lambda).
#[derive(Debug, Default)]
struct FnScope {
    /// Block scopes, innermost last.
    scopes: Vec<Scope>,
    /// What this function captures (index = upvalue index).
    captures: Vec<Capture>,
    /// For a local fn: its own name and binding (`Res::SelfRef`).
    self_name: Option<(String, BindingId)>,
}

/// Key of a known signature: a global slot or a local binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SigKey {
    Global(GlobalId),
    Binding(BindingId),
}

fn sig_key(r: Res) -> SigKey {
    match r {
        Res::Global(g) => SigKey::Global(g),
        Res::Local(b) | Res::Upvalue { binding: b, .. } | Res::SelfRef(b) => SigKey::Binding(b),
    }
}

/// While resolving a fn or lambda body: what it may see.
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
    /// The functions being resolved, outermost (`<main>`) first: each
    /// nested fn or lambda pushes one.
    funcs: Vec<FnScope>,
    /// Set while resolving a fn or lambda body.
    fn_ctx: Option<FnCtx>,
    /// Known signatures.
    sigs: Vec<Sig>,
    /// Name target → index into `sigs`.
    known: HashMap<SigKey, usize>,
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
                SigKey::Global(g),
                Sig {
                    name: n.name.to_string(),
                    params: n
                        .params
                        .iter()
                        .map(|p| (p.name.to_string(), p.has_default))
                        .collect(),
                    positional: n
                        .params
                        .iter()
                        .filter(|p| !p.has_default || p.positional)
                        .count(),
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
        self.add_sig(SigKey::Global(g), fn_sig(d));
    }

    fn new_global(&mut self, name: &str, span: Span, slot: GlobalSlot) -> GlobalId {
        self.out.globals.push(GlobalInfo {
            name: name.to_string(),
            span,
            slot,
        });
        GlobalId(self.out.globals.len() as u32 - 1)
    }

    fn add_sig(&mut self, key: SigKey, sig: Sig) {
        self.sigs.push(sig);
        self.known.insert(key, self.sigs.len() - 1);
    }

    /// The function being resolved.
    fn func(&mut self) -> &mut FnScope {
        self.funcs.last_mut().expect("<main> is always there")
    }

    /// The innermost block scope (there is one inside every fn body and
    /// block).
    fn scope(&mut self) -> &mut Scope {
        self.func().scopes.last_mut().expect("a block has a scope")
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
            Item::Fn(d) if top_level => self.fn_def(d, None),
            Item::Fn(d) => {
                // A local fn (S21): bound in the block from here on, before
                // its body is resolved, so the body can call itself.
                let id = self.new_local(&d.name, d.name_span);
                let scope = self.scope();
                if let Some(i) = scope.later_fns.iter().position(|(n, _)| *n == d.name) {
                    scope.later_fns.remove(i);
                }
                scope.names.push((d.name.clone(), id));
                self.out.lets.insert(d.id, Some(Res::Local(id)));
                self.add_sig(SigKey::Binding(id), fn_sig(d));
                self.fn_def(d, Some(id));
            }
        }
    }

    /// `let`: annotation and initialiser first (the initialiser sees the
    /// *previous* binding of the same name), then the new binding.
    fn binding(&mut self, b: &Binding, top_level: bool) {
        if let Some(t) = &b.ann {
            self.check_type(t);
        }
        self.expr(&b.init);
        // `let g = f` gives `g` the signature of `f`; `let g = x => …` and
        // `let g = f(1, _)` the lambda's.
        let sig = self.sig_of(&b.init, b);
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
            self.scope().names.push((b.name.clone(), id));
            Res::Local(id)
        };
        self.out.lets.insert(b.id, Some(res));
        if let Some(i) = sig {
            self.known.insert(sig_key(res), i);
        }
    }

    /// The signature an initialiser gives the binding `b`: that of a named
    /// callee, of a lambda, or of the lambda at the end of a block (a `_`
    /// partial after desugaring).
    fn sig_of(&mut self, init: &Expr, b: &Binding) -> Option<usize> {
        match &init.kind {
            ExprKind::Name(_) => self
                .out
                .names
                .get(&init.id)
                .and_then(|r| self.known.get(&sig_key(*r)))
                .copied(),
            ExprKind::Lambda(l) => {
                self.sigs.push(Sig {
                    name: b.name.clone(),
                    params: l.params.iter().map(|p| (p.name.clone(), false)).collect(),
                    positional: l.params.len(),
                    def: Some(b.name_span),
                });
                Some(self.sigs.len() - 1)
            }
            ExprKind::Block(items) => match items.last() {
                Some(Item::Expr(e)) => self.sig_of(e, b),
                _ => None,
            },
            _ => None,
        }
    }

    /// A fn (top-level, or local with binding `local`): parameters become
    /// locals of a new `FnScope`; the body sees the top-level `let`s defined
    /// before the fn (plus later ones as forward references, see `lookup`)
    /// and, for a local fn, the enclosing function's locals (captures).
    fn fn_def(&mut self, d: &FnDef, local: Option<BindingId>) {
        let mut type_params: Vec<String> = match (&self.fn_ctx, local) {
            (Some(c), Some(_)) => c.type_params.clone(),
            _ => Vec::new(),
        };
        type_params.extend(d.type_params.iter().map(|(n, _)| n.clone()));
        let saved_ctx = self.fn_ctx.replace(FnCtx {
            top_len: self.visible_lets(),
            type_params,
        });
        self.funcs.push(FnScope {
            self_name: local.map(|b| (d.name.clone(), b)),
            ..FnScope::default()
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
        // Parameters are added one by one, so a default sees only the
        // parameters before it (S19).
        self.func().scopes.push(Scope::default());
        self.params(&d.params, &d.name);
        if let Some(t) = &d.ret {
            self.check_type(t);
        }
        self.expr(&d.body);
        let f = self.funcs.pop().expect("pushed above");
        if local.is_some() {
            self.out.captures.insert(
                d.id,
                FnCaptures {
                    name: d.name.clone(),
                    span: d.name_span,
                    list: f.captures,
                },
            );
        }
        self.fn_ctx = saved_ctx;
    }

    /// Declare parameters (of a fn or lambda) in the current scope, in
    /// order; resolve their annotations and defaults. `_` binds nothing.
    fn params(&mut self, params: &[Param], fn_name: &str) {
        for p in params {
            if let Some(t) = &p.ann {
                self.check_type(t);
            }
            if let Some(default) = &p.default {
                self.expr(default);
            }
            if p.name == "_" {
                self.out.lets.insert(p.id, None);
                continue;
            }
            let first = self
                .scope()
                .names
                .iter()
                .find(|(n, _)| *n == p.name)
                .map(|(_, b)| *b);
            if let Some(first) = first {
                let first_span = self.out.locals[first.0 as usize].span;
                self.diags.push(
                    Diagnostic::new(
                        "E0207",
                        format!("parameter `{}` is defined twice in {fn_name}", p.name),
                        p.span,
                    )
                    .label("second definition")
                    .with(first_span, "first defined here"),
                );
            }
            let id = self.new_local(&p.name, p.span);
            self.out.lets.insert(p.id, Some(Res::Local(id)));
            self.scope().names.push((p.name.clone(), id));
        }
    }

    /// A lambda (S3): like a fn body, without a name or a return type. It
    /// sees what the code around it sees (captures, type variables).
    fn lambda(&mut self, e: &Expr, l: &Lambda) {
        let type_params = self
            .fn_ctx
            .as_ref()
            .map(|c| c.type_params.clone())
            .unwrap_or_default();
        let saved_ctx = self.fn_ctx.replace(FnCtx {
            top_len: self.visible_lets(),
            type_params,
        });
        self.funcs.push(FnScope::default());
        self.func().scopes.push(Scope::default());
        self.params(&l.params, "lambda");
        self.expr(&l.body);
        let f = self.funcs.pop().expect("pushed above");
        self.out.captures.insert(
            e.id,
            FnCaptures {
                name: "lambda".into(),
                span: e.span,
                list: f.captures,
            },
        );
        self.fn_ctx = saved_ctx;
    }

    /// How many top-level `let`s the code being resolved can see.
    fn visible_lets(&self) -> usize {
        self.fn_ctx.as_ref().map_or(self.lets_done, |c| c.top_len)
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
                let later_fns = items
                    .iter()
                    .filter_map(|i| match i {
                        Item::Fn(d) => Some((d.name.clone(), d.name_span)),
                        _ => None,
                    })
                    .collect();
                self.func().scopes.push(Scope {
                    names: Vec::new(),
                    later_fns,
                });
                for item in items {
                    self.item(item, false);
                }
                self.func().scopes.pop();
            }
            ExprKind::Call { callee, args, .. } => self.call(e, callee, args),
            ExprKind::Lambda(l) => self.lambda(e, l),
            ExprKind::Str(_) | ExprKind::None => {}
            ExprKind::Try(x) => self.expr(x),
            ExprKind::Interp(parts) => {
                for p in parts {
                    if let StrPart::Expr(x) = p {
                        self.expr(x);
                    }
                }
            }
            ExprKind::Pipe { .. } => unreachable!("pipes are removed by desugar.rs"),
        }
    }

    /// A call: resolve callee and arguments. When the callee has a known
    /// signature, check the arguments against it and record how they map
    /// onto the parameters (`Resolved::calls`); otherwise only positional
    /// arguments are allowed (E0422).
    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Arg]) {
        self.expr(callee);
        for a in args {
            match a {
                Arg::Positional(x) | Arg::Keyword { value: x, .. } => self.expr(x),
                // `desugar.rs` has turned every `_` into a lambda parameter.
                Arg::Placeholder(_) => unreachable!("placeholders are removed by desugar.rs"),
            }
        }
        let sig = match callee.kind {
            ExprKind::Name(_) => self
                .out
                .names
                .get(&callee.id)
                .and_then(|r| self.known.get(&sig_key(*r)))
                .copied(),
            _ => None,
        };
        match sig {
            Some(i) => {
                if let Some(plan) = self.plan_call(e, i, args) {
                    self.out.calls.insert(e.id, plan);
                }
            }
            None => {
                for a in args {
                    if let Arg::Keyword { name_span, .. } = a {
                        self.diags.push(
                            Diagnostic::new(
                                "E0422",
                                "keyword arguments need a function known at compile time",
                                *name_span,
                            )
                            .label("the callee here is a computed value")
                            .help("call a top-level fn or a built-in by its name, or pass the arguments by position"),
                        );
                    }
                }
            }
        }
    }

    /// Match `args` against signature `sigs[i]` (S19): positional arguments
    /// fill the required parameters in order, keyword arguments name the
    /// defaulted ones. Reports E0205, E0421, E0424, E0425; `None` on error.
    fn plan_call(&mut self, e: &Expr, i: usize, args: &[Arg]) -> Option<CallPlan> {
        let sig = self.sigs[i].clone();
        let required = sig.params.iter().filter(|(_, d)| !d).count();
        let mut slots: Vec<Option<usize>> = vec![None; sig.params.len()];
        let mut positional = 0;
        // Required parameters wrongly passed by name (E0424) still count as
        // supplied, so the same mistake is not also reported as E0205.
        let mut named_required = 0;
        let mut ok = true;
        for (j, a) in args.iter().enumerate() {
            match a {
                Arg::Positional(x) => {
                    positional += 1;
                    let k = positional - 1;
                    if k >= sig.positional && k < sig.params.len() {
                        let name = &sig.params[k].0;
                        self.diags.push(
                            Diagnostic::new(
                                "E0421",
                                format!("`{name}` has a default and must be passed by name"),
                                x.span,
                            )
                            .label(format!("passed by position as parameter {}", k + 1))
                            .help(format!(
                                "write `{name}=…`: parameters with a default are keyword-only"
                            )),
                        );
                        ok = false;
                    } else if k < sig.params.len() {
                        slots[k] = Some(j);
                    }
                }
                Arg::Keyword {
                    name, name_span, ..
                } => match sig.params.iter().position(|(p, _)| p == name) {
                    None => {
                        let names: Vec<&str> = sig
                            .params
                            .iter()
                            .filter(|(_, d)| *d)
                            .map(|(p, _)| p.as_str())
                            .collect();
                        let mut d = Diagnostic::new(
                            "E0424",
                            format!("`{}` has no parameter named `{name}`", sig.name),
                            *name_span,
                        )
                        .label("unknown keyword");
                        if let Some(def) = sig.def {
                            d = d.with(def, "defined here");
                        }
                        d = match suggest(name, &names) {
                            Some(s) => d.help(format!("did you mean `{s}`?")),
                            None if names.is_empty() => {
                                d.help(format!("`{}` takes no keyword arguments", sig.name))
                            }
                            None => {
                                d.help(format!("the keyword parameters are: {}", names.join(", ")))
                            }
                        };
                        self.diags.push(d);
                        ok = false;
                    }
                    Some(k) if k < sig.positional => {
                        named_required += 1;
                        self.diags.push(
                            Diagnostic::new(
                                "E0424",
                                format!("`{name}` is a required parameter of `{}`", sig.name),
                                *name_span,
                            )
                            .label("passed by name")
                            .help(format!(
                                "pass `{name}` by position; only parameters with a default are passed by name"
                            )),
                        );
                        ok = false;
                    }
                    Some(k) if slots[k].is_some() => {
                        self.diags.push(
                            Diagnostic::new(
                                "E0425",
                                format!("keyword argument `{name}` is given twice"),
                                *name_span,
                            )
                            .label("second time")
                            .with(arg_span(&args[slots[k].unwrap_or(0)]), "first time"),
                        );
                        ok = false;
                    }
                    Some(k) => slots[k] = Some(j),
                },
                Arg::Placeholder(_) => unreachable!("placeholders are removed by desugar.rs"),
            }
        }
        let supplied = positional + named_required;
        if supplied < required || positional > sig.params.len() {
            self.wrong_count(e, &sig, required, positional);
            return None;
        }
        if !ok {
            return None;
        }
        // Trailing omitted parameters are padded by the VM.
        while slots.last() == Some(&None) {
            slots.pop();
        }
        Some(CallPlan {
            slots: slots
                .into_iter()
                .map(|s| s.map_or(Slot::Missing, Slot::Arg))
                .collect(),
        })
    }

    /// E0205: wrong number of positional arguments to a known callee.
    fn wrong_count(&mut self, e: &Expr, sig: &Sig, want: usize, given: usize) {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let what = if want == sig.params.len() || sig.positional == sig.params.len() {
            "argument"
        } else {
            "positional argument"
        };
        // `range` takes 1 to 2 arguments (Q26).
        let takes = if sig.positional > want {
            format!("{want} to {} {what}s", sig.positional)
        } else {
            format!("{want} {what}{}", plural(want))
        };
        let mut d = Diagnostic::new(
            "E0205",
            format!(
                "`{}` takes {takes} but {given} {} given",
                sig.name,
                if given == 1 { "was" } else { "were" }
            ),
            e.span,
        )
        .label(format!("called with {given} {what}{}", plural(given)));
        if let Some(def) = sig.def {
            d = d.with(def, "defined here");
        }
        if given < want {
            let shown: Vec<&str> = sig.params[..want]
                .iter()
                .enumerate()
                .map(|(j, (p, _))| if j < given { p.as_str() } else { "_" })
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
                    .help("`_` discards a value (`let _ = …`) or stands for a missing argument in a call (`f(a, _)`); it is not a variable"),
            );
            return;
        }
        match self.lookup(name, e.span) {
            Some(res) => {
                self.out.names.insert(e.id, res);
                // Names made up by `desugar.rs` (`$a0`) are not user uses.
                if !name.starts_with('$') {
                    self.out.uses.push(Use {
                        span: e.span,
                        name: name.to_string(),
                        res,
                    });
                }
            }
            None => self.undefined(e.span, name),
        }
    }

    /// Find what `name` refers to here (order: see the module docs). `span`
    /// is the use, for the capture-limit error.
    fn lookup(&mut self, name: &str, span: Span) -> Option<Res> {
        if let Some(r) = self.lookup_in(self.funcs.len() - 1, name, span) {
            return Some(r);
        }
        let visible = self.visible_lets();
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

    /// Look `name` up among the locals of `funcs[level]`, then of the
    /// functions around it. A local of an enclosing function is captured by
    /// every function in between (Lua `singlevar`).
    fn lookup_in(&mut self, level: usize, name: &str, span: Span) -> Option<Res> {
        let f = &self.funcs[level];
        // RUST: `find_map` returns the first `Some` produced by the closure.
        let local = f.scopes.iter().rev().find_map(|s| {
            s.names
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, b)| *b)
        });
        if let Some(b) = local {
            return Some(Res::Local(b));
        }
        if let Some((n, b)) = &f.self_name {
            if n == name {
                return Some(Res::SelfRef(*b));
            }
        }
        if level == 0 {
            return None;
        }
        let (binding, from) = match self.lookup_in(level - 1, name, span)? {
            Res::Local(b) => (b, CaptureFrom::Local(b)),
            Res::Upvalue { binding, index } => (binding, CaptureFrom::Upvalue(index)),
            Res::SelfRef(b) => (b, CaptureFrom::SelfFn),
            Res::Global(_) => unreachable!("lookup_in only finds locals"),
        };
        let captures = &mut self.funcs[level].captures;
        let index = match captures.iter().position(|c| c.binding == binding) {
            Some(i) => i,
            None => {
                captures.push(Capture { binding, from });
                captures.len() - 1
            }
        };
        if index > u8::MAX as usize {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                self.diags.push(
                    Diagnostic::new("E0301", "too many captured variables", span)
                        .label("a function can capture at most 256 variables"),
                );
            }
            return Some(Res::Upvalue { binding, index: 0 });
        }
        Some(Res::Upvalue {
            binding,
            index: index as u8,
        })
    }

    /// E0203 if a top-level `let` of this name comes later; E0204/E0201 if
    /// a local fn of an enclosing block comes later; else E0201.
    fn undefined(&mut self, span: Span, name: &str) {
        // A later local fn of an enclosing block: from inside another
        // function (a local fn or lambda) that is local mutual recursion.
        let current = self.funcs.len() - 1;
        let later = self.funcs.iter().enumerate().rev().find_map(|(level, f)| {
            f.scopes.iter().rev().find_map(|s| {
                s.later_fns
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, d)| (level, *d))
            })
        });
        if let Some((level, def)) = later {
            let d = if level < current {
                Diagnostic::new(
                    "E0204",
                    format!("`{name}` is a local fn defined later in this block"),
                    span,
                )
                .label("used here")
                .with(def, "defined here")
                .help("local fns cannot call each other (mutual recursion); define both fns at the top level of the file")
            } else {
                Diagnostic::new("E0201", format!("undefined name `{name}`"), span)
                    .label("used here")
                    .with(def, "defined later in this block")
                    .help("move the local fn above this line")
            };
            self.diags.push(d);
            return;
        }
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
            .chain(
                self.funcs
                    .iter()
                    .flat_map(|f| f.scopes.iter().flat_map(|s| &s.names))
                    .map(|(n, _)| n.as_str())
                    .filter(|n| !n.starts_with('$')),
            )
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
        // (`fn_ctx` holds the type variables of every enclosing fn.)
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

/// Source span of a call argument.
fn arg_span(a: &Arg) -> Span {
    match a {
        Arg::Positional(x) | Arg::Keyword { value: x, .. } => x.span,
        Arg::Placeholder(s) => *s,
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

/// The signature of a fn definition.
fn fn_sig(d: &FnDef) -> Sig {
    Sig {
        name: d.name.clone(),
        params: d
            .params
            .iter()
            .map(|p| (p.name.clone(), p.default.is_some()))
            .collect(),
        positional: d.params.iter().filter(|p| p.default.is_none()).count(),
        def: Some(d.name_span),
    }
}

fn res_str(r: Res) -> String {
    match r {
        Res::Local(BindingId(i)) => format!("local#{i}"),
        Res::Global(GlobalId(i)) => format!("global#{i}"),
        Res::Upvalue {
            binding: BindingId(b),
            index,
        } => format!("upval#{index} (local#{b})"),
        Res::SelfRef(BindingId(b)) => format!("self (local#{b})"),
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
    if !r.captures.is_empty() {
        out.push_str("captures:\n");
        let mut fns: Vec<&FnCaptures> = r.captures.values().collect();
        fns.sort_by_key(|f| f.span.start);
        for f in fns {
            let (l, c) = sm.line_col(f.span.start);
            let list: Vec<String> = f
                .list
                .iter()
                .enumerate()
                .map(|(i, cap)| {
                    let from = match cap.from {
                        CaptureFrom::Local(BindingId(b)) => format!("local#{b}"),
                        CaptureFrom::Upvalue(u) => format!("upval#{u}"),
                        CaptureFrom::SelfFn => "self".to_string(),
                    };
                    format!("#{i} {} <- {from}", r.locals[cap.binding.0 as usize].name)
                })
                .collect();
            let list = if list.is_empty() {
                "(none)".to_string()
            } else {
                list.join(", ")
            };
            let _ = writeln!(out, "  {:<12} {:<7} {list}", f.name, format!("{l}:{c}"));
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
        assert!(d.contains("2:17    x -> global#32"), "{d}");
    }

    // Snapshot: src/snapshots/valo_compiler__resolve__tests__capture_chain.snap
    #[test]
    fn capture_chain() {
        // `a` is captured by `mid` and, through it, by the lambda (3
        // levels); `mid` inside the lambda is the lambda's capture of
        // `mid`'s own closure.
        insta::assert_snapshot!(
            resolve_src(
                "fn outer(a: int) -> int:\n    fn mid(b: int) -> int:\n        \
                 let f = c => a + b + c + mid(0)\n        f(1)\n    mid(10)"
            )
            .unwrap()
        );
    }

    #[test]
    fn local_fns_and_lambdas() {
        let ok = |s: &str| resolve_src(s).unwrap();
        // A local fn calling itself is `self`, not a capture.
        let d = ok(
            "fn f() -> int:\n    fn g(n: int) -> int = if n == 0 then 0 else g(n - 1)\n    g(3)",
        );
        assert!(d.contains("g -> self (local#0)"), "{d}");
        // Globals are never captured.
        let d = ok("let k = 1\nlet f = x => x + k");
        assert!(d.contains("k -> global#"), "{d}");
        // Keyword arguments to a local fn with defaults.
        assert!(
            resolve_src(
                "fn f() -> int:\n    fn g(a: int, b: int = 2) -> int = a + b\n    g(1, b=3)"
            )
            .is_ok()
        );
        // A lambda parameter `_` binds nothing.
        assert_eq!(resolve_src("let f = (_, y) => _").unwrap_err(), ["E0206"]);
    }

    #[test]
    fn keyword_arguments() {
        let codes = |s: &str| -> Vec<&'static str> { resolve_src(s).unwrap_err() };
        let f = "fn f(a: int, b: int = 1, c: int = 2) -> int = a + b + c\n";
        assert!(resolve_src(&format!("{f}f(1)\nf(1, c=3)\nf(1, c=3, b=4)\n5 |> f(b=1)")).is_ok());
        assert_eq!(codes(&format!("{f}f(1, 2)")), ["E0421"]);
        assert_eq!(codes(&format!("{f}f(1, 2, 3)")), ["E0421", "E0421"]);
        assert_eq!(codes(&format!("{f}f(1, d=2)")), ["E0424"]);
        assert_eq!(codes(&format!("{f}f(1, a=2)")), ["E0424"]);
        assert_eq!(codes(&format!("{f}f(1, b=2, b=3)")), ["E0425"]);
        assert_eq!(codes(&format!("{f}f(b=2)")), ["E0205"]);
        assert_eq!(
            codes(&format!("{f}let h = if true then f else f\nh(1, b=2)")),
            ["E0422"]
        );
        // Defaults see earlier parameters only.
        assert_eq!(codes("fn g(a: int = b, b: int = 1) -> int = a"), ["E0201"]);
        assert!(resolve_src("fn g(a: int, b: int = a * 2) -> int = b").is_ok());
        // Built-ins go through the same signature check.
        assert_eq!(codes("print(1, sep=2)"), ["E0424"]);
        assert!(resolve_src("print(1, end=\"\")").is_ok());
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
        // Local fns (M6): visible from their line on; no mutual recursion.
        assert!(resolve_src("let v = if true:\n    fn g() -> int = 1\n    g()\nelse: 0").is_ok());
        assert_eq!(
            codes("let v = if true:\n    let t = g()\n    fn g() -> int = 1\n    t\nelse: 0"),
            ["E0201"]
        );
        assert_eq!(
            codes("fn f() -> int:\n    fn g() -> int = h()\n    fn h() -> int = 1\n    g()"),
            ["E0204"]
        );
        // `_` partials (M6) get the lambda's signature.
        assert!(resolve_src("fn f(x: int) -> int = x\nf(_)").is_ok());
        assert_eq!(
            codes("fn f(x: int, y: int) -> int = x\nlet g = f(1, _)\ng(1, 2)"),
            ["E0205"]
        );
        assert_eq!(codes("let g = (a, b) => a\ng(1)"), ["E0205"]);
        assert_eq!(codes("let g = (a, a) => a"), ["E0207"]);
        assert_eq!(codes("fn f(x: int) -> int = x\nf(x=1)"), ["E0424"]);
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
