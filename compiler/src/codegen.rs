//! # Code generation (`codegen.rs`)
//!
//! **What this file is:** the last compiler stage. It walks the syntax tree
//! and emits register-machine instructions (`valo_vm::opcode::Instr`) into a
//! function prototype (`valo_vm::chunk::Proto`), deciding which register holds
//! each intermediate value.
//!
//! **Pipeline position:** `ast::Program` + `resolve::Resolved` in →
//! `valo_vm::chunk::Program` out → `Vm::run`. Entry point: `compile`.
//!
//! **Textbook chapter:** `docs/06-code-generation.md`.
//!
//! ## Technique (Lua `lcode.c` style, plan D1)
//!
//! - `FuncState::expr(e, dst)` compiles expression `e` so that its value ends
//!   up in register `dst`. The caller chooses `dst`.
//! - Temporaries are allocated stack-like above `dst` (`alloc`) and released
//!   in reverse order (`free_to`), so the register count stays small.
//! - Every emitted instruction records the span of the AST node responsible
//!   for it (`emit`), which is how runtime errors know their source location.
//!
//! Example: `(1 + 2) * 3` into R0
//!
//! ```text
//! LOADI 0 1     ; R0 = 1          lhs of + into dst
//! LOADI 1 2     ; R1 = 2          rhs of + into a temporary
//! ADD   0 0 1   ; R0 = R0 + R1    R1 freed
//! LOADI 1 3     ; R1 = 3          rhs of * reuses R1
//! MUL   0 0 1   ; R0 = R0 * R1
//! ```
//!
//! ## Where to change things
//!
//! | I want to…                                  | Look at                                          |
//! |---------------------------------------------|--------------------------------------------------|
//! | compile a new expression kind               | `FuncState::expr`, add a `match` arm              |
//! | compile a new top-level item (`def`)        | `compile`, the `for item in &program.items` loop  |
//! | change how a function is compiled           | `compile_function` (parameters, body, `RETURN`)   |
//! | change how closures capture (M6)            | `FuncState::closure` (descriptors), `FuncState::lambda` (names) |
//! | change how calls are compiled               | `FuncState::call` (the argument window)           |
//! | change how defaults are computed            | the prologue loop in `compile_function` (`JMPIFSET`) |
//! | change what counts as a tail position       | `FuncState::expr_at` and its callers (`if_expr`, `block`) |
//! | change how names are loaded                 | `FuncState::name` (`GETGLOBAL`, `MOVE`, `GETUPVAL`, `GETSELF`) |
//! | map an operator to an opcode                | `ExprKind::Binary` arm (`let opcode = match op`) |
//! | add a constant type (string, M7)            | `ConstKey` + `FuncState::constant` + `Const` in `vm/src/chunk.rs` |
//! | compile a branch / jump                     | `emit_jmp` + `patch_here` (backpatching); see `if_expr`, `logic`, `compare` |
//! | change how blocks use registers             | `FuncState::block`                               |
//! | add an optimisation (constant folding…)     | `FuncState::expr` (see the `-<literal>` fold)    |
//! | change register limits                      | `MAX_REGS`, `FuncState::alloc`                   |
//! | change debug info for locals (debugger, D3)  | `FuncState::declare_local`, `close_locals`        |
//!
//! ## Functions and calls (M4)
//!
//! - Every `fn` and lambda becomes its own `Proto`, compiled by its own
//!   `FuncState` (`compile_function`); top-level fns are children of
//!   `<main>`, local fns and lambdas of the function they appear in
//!   (`Proto::protos`). Parameters are `R0..R(arity-1)`.
//! - Closures (M6): the enclosing function turns the resolver's capture
//!   list into `Proto::upvals` (where `CLOSURE` copies each value from); a
//!   captured name compiles to `GETUPVAL`, a local fn's own name to
//!   `GETSELF`.
//! - Top-level fns are hoisted (plan D4): `<main>` starts with
//!   `CLOSURE r k; SETGLOBAL r g` for each of them, before any other code.
//! - A call reserves a *window* of consecutive registers
//!   `[base, base+1, …, base+n]` first — callee, then the arguments — and
//!   only then evaluates into it (`FuncState::call`). `CALL base n` leaves
//!   the result in `base`.
//!
//! ```text
//! f(1, g(2))  into R0       R0 = window base (dst is the top register)
//!   GETGLOBAL 0 f           callee
//!   LOADI     1 1           argument 1
//!   GETGLOBAL 2 g           argument 2 = inner call, window [2, 3]
//!   LOADI     3 2
//!   CALL      2 1           R2 = g(2)
//!   CALL      0 2           R0 = f(R1, R2)
//! ```
//!
//! Code generation error codes: E0301 limit exceeded, E0009 not supported yet.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `HashMap<i64, u16>` — dictionary for constant deduplication (*Structs*).
//! - `fn finish(mut self)` — consumes the `FuncState` and returns its parts
//!   (*Ownership and borrowing*).
//! - `i16::try_from(n)` — checked narrowing conversion (*Integer types and casts*).
//! - `let Ok(k) = ... else { ... }` (*Pattern matching*).

use std::collections::HashMap;
use std::rc::Rc;

use valo_vm::chunk::{
    Const, GlobalDesc, GlobalKind, LocalInfo, NativeId, Program, Proto, SrcSpan, UpvalDesc,
    UpvalSrc,
};
use valo_vm::opcode::{Instr, MAX_SJ, MIN_SJ, OpCode};

use crate::ast::{
    self, Arg, BinOp, Binding, CmpLink, CmpOp, Expr, ExprKind, Item, Lambda, LogicOp, NodeId,
    Param, UnaryOp,
};
use crate::diagnostic::Diagnostic;
use crate::resolve::{BindingId, CaptureFrom, GlobalId, GlobalSlot, Res, Resolved, Slot};
use crate::span::{SourceMap, Span};

/// Code generation options.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Emit `ECHO` after every top-level expression (`valo run --echo`).
    pub echo: bool,
}

/// Compile a parsed and resolved program into the `<main>` prototype.
///
/// `<main>` first creates every top-level fn (`CLOSURE` + `SETGLOBAL`,
/// hoisting, plan D4). Then every top-level expression is evaluated into a
/// temporary (and echoed with `--echo`); every top-level `let` stores its
/// value into a global slot (`SETGLOBAL`). The program ends with `LOADUNIT`
/// + `RETURN`.
pub fn compile(
    program: &ast::Program,
    resolved: &Resolved,
    sm: &SourceMap,
    opts: Options,
) -> Result<Program, Vec<Diagnostic>> {
    let mut fs = FuncState::new("<main>", Span::new(0, 0), resolved, sm);
    let mut diags = Vec::new();
    for item in &program.items {
        let Item::Fn(d) = item else { continue };
        let child = match compile_function(&d.name, d.name_span, &d.params, &d.body, resolved, sm) {
            Ok(p) => p,
            Err(ds) => {
                diags.extend(ds);
                continue;
            }
        };
        let Some(Some(Res::Global(g))) = resolved.lets.get(&d.id).copied() else {
            // A duplicate fn (E0207) has no slot; resolution failed anyway.
            continue;
        };
        let k = fs.child(child, d.name_span);
        let r = fs.alloc(d.name_span);
        fs.emit(Instr::abx(OpCode::CLOSURE, r, k), d.name_span);
        let bx = fs.global_index(g, d.name_span);
        fs.emit(Instr::abx(OpCode::SETGLOBAL, r, bx), d.name_span);
        fs.free_to(r);
    }
    // EXTEND: new `Item` kinds (def, ...) get arms here.
    for item in &program.items {
        match item {
            // Already created by the prologue above.
            Item::Fn(_) => {}
            Item::Expr(e) => {
                let dst = fs.alloc(e.span);
                fs.expr(e, dst);
                if opts.echo {
                    fs.emit(Instr::abc(OpCode::ECHO, dst, 0, 0), e.span);
                }
                fs.free_to(dst);
            }
            Item::Let(b) => {
                let tmp = fs.alloc(b.init.span);
                fs.let_init(b, tmp);
                // `let _ = …` has no slot: the value is evaluated and
                // discarded. (Top-level `let`s are never locals.)
                if let Some(Res::Global(g)) = resolved.lets.get(&b.id).copied().flatten() {
                    let bx = fs.global_index(g, b.name_span);
                    fs.emit(Instr::abx(OpCode::SETGLOBAL, tmp, bx), b.name_span);
                }
                fs.free_to(tmp);
            }
        }
    }
    let end = Span::new(sm.text.len(), sm.text.len());
    let r = fs.alloc(end);
    fs.emit(Instr::abc(OpCode::LOADUNIT, r, 0, 0), end);
    fs.emit(Instr::abc(OpCode::RETURN, r, 0, 0), end);
    let main = fs.finish();
    let main = match main {
        Ok(m) if diags.is_empty() => m,
        Ok(_) => return Err(diags),
        Err(ds) => {
            diags.extend(ds);
            return Err(diags);
        }
    };
    Ok(Program {
        main: Rc::new(main),
        globals: resolved
            .globals
            .iter()
            .map(|g| GlobalDesc {
                name: g.name.as_str().into(),
                kind: match g.slot {
                    GlobalSlot::Native(i) => GlobalKind::Native(NativeId(i)),
                    GlobalSlot::Fn | GlobalSlot::Let => GlobalKind::User,
                },
            })
            .collect(),
        source_name: sm.name.as_str().into(),
        line_starts: sm.line_starts.clone(),
    })
}

/// Compile one function (a `fn`, a local `fn` or a lambda) into its own
/// prototype: parameters in `R0..`, the body into the next register, then
/// `RETURN`. Its captures (`Proto::upvals`) are filled in by the enclosing
/// function (`FuncState::closure`), which knows their registers.
fn compile_function(
    name: &str,
    def_span: Span,
    params: &[Param],
    body: &Expr,
    resolved: &Resolved,
    sm: &SourceMap,
) -> Result<Proto, Vec<Diagnostic>> {
    let mut fs = FuncState::new(name, def_span, resolved, sm);
    for p in params {
        let r = fs.alloc(p.span);
        if let Some(Some(Res::Local(b))) = resolved.lets.get(&p.id).copied() {
            fs.declare_local(b, r);
        }
    }
    fs.proto.arity = params.len().min(255) as u8;
    fs.proto.required = params
        .iter()
        .filter(|p| p.default.is_none())
        .count()
        .min(255) as u8;
    // Prologue (M4b): a parameter the caller omitted holds `Missing`;
    // compute its default into its own register. Defaults run in order, so
    // each may use the parameters before it.
    //     JMPIFSET p +n     ; passed: skip the default
    //     <default → Rp>    ; n instructions
    for (i, p) in params.iter().enumerate() {
        let Some(default) = &p.default else { continue };
        let at = fs.proto.code.len();
        fs.emit(Instr::asbx(OpCode::JMPIFSET, i as u8, 0), p.span);
        fs.expr(default, i as u8);
        fs.patch_sbx(at);
    }
    let dst = fs.alloc(body.span);
    // The body is in tail position: a call there becomes TAILCALL (M5).
    fs.expr_at(body, dst, true);
    fs.emit(Instr::abc(OpCode::RETURN, dst, 0, 0), body.span);
    fs.finish()
}

/// Per-function compilation state (Lua `FuncState`). One per function being
/// compiled (`compile` for `<main>`, `compile_function` for each fn and
/// lambda).
// RUST: `'r` ties the borrowed resolver output to the FuncState's lifetime.
struct FuncState<'r> {
    /// Name resolution results (which slot each name refers to).
    resolved: &'r Resolved,
    /// Source text, for lambda names (`<lambda@3:9>`).
    sm: &'r SourceMap,
    /// Register holding each local binding currently in scope.
    locals: HashMap<BindingId, u8>,
    /// The prototype being filled in.
    proto: Proto,
    /// First free register.
    free_reg: u16,
    /// High-water mark of `free_reg` = number of registers the frame needs.
    max_reg: u16,
    /// Value → index in `proto.consts`, so each constant is stored once.
    consts: HashMap<ConstKey, u16>,
    /// Errors found while generating code.
    diags: Vec<Diagnostic>,
}

/// Constant-pool deduplication key. Floats are compared by bit pattern, so
/// `0.0` and `-0.0` stay distinct (and `NaN` would be a valid key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ConstKey {
    Int(i64),
    Float(u64),
}

/// Registers are 8-bit operands.
const MAX_REGS: u16 = 255;

/// `LocalInfo::end_pc` of a local whose scope has not ended yet.
const OPEN: u32 = u32::MAX;

impl<'r> FuncState<'r> {
    fn new(name: &str, def_span: Span, resolved: &'r Resolved, sm: &'r SourceMap) -> FuncState<'r> {
        FuncState {
            resolved,
            sm,
            locals: HashMap::new(),
            proto: Proto {
                name: name.into(),
                arity: 0,
                required: 0,
                num_regs: 0,
                code: Vec::new(),
                consts: Vec::new(),
                spans: Vec::new(),
                def_span: def_span.into(),
                protos: Vec::new(),
                locals: Vec::new(),
                upvals: Vec::new(),
            },
            free_reg: 0,
            max_reg: 0,
            consts: HashMap::new(),
            diags: Vec::new(),
        }
    }

    /// Bind a local to register `r` from the next instruction on, and record
    /// it in the debug info (`Proto::locals`). Its range stays open until
    /// `close_locals` (end of block) or `finish` (parameters).
    fn declare_local(&mut self, b: BindingId, r: u8) {
        self.locals.insert(b, r);
        let name = self.resolved.locals[b.0 as usize].name.as_str();
        // Temporaries of a `_` partial (`$a0`) are not user variables.
        if name.starts_with('$') {
            return;
        }
        let name = name.into();
        self.proto.locals.push(LocalInfo {
            name,
            reg: r,
            start_pc: self.proto.code.len() as u32,
            end_pc: OPEN,
        });
    }

    /// End the scope of every local declared since `first` that is still open.
    fn close_locals(&mut self, first: usize) {
        let pc = self.proto.code.len() as u32;
        for l in &mut self.proto.locals[first..] {
            if l.end_pc == OPEN {
                l.end_pc = pc;
            }
        }
    }

    /// Finish the prototype, or return the errors collected on the way.
    // RUST: `mut self` (no `&`) consumes the FuncState; it cannot be used
    // after `finish`, which is exactly right.
    fn finish(mut self) -> Result<Proto, Vec<Diagnostic>> {
        if !self.diags.is_empty() {
            return Err(self.diags);
        }
        self.close_locals(0); // parameters live to the end
        self.proto.num_regs = self.max_reg.max(1) as u8;
        Ok(self.proto)
    }

    /// Append one instruction and remember which source produced it.
    fn emit(&mut self, instr: Instr, span: Span) {
        self.proto.code.push(instr);
        self.proto.spans.push(SrcSpan::from(span));
    }

    /// Reserve the next register. On overflow, report E0301 once and keep
    /// going with a dummy register so code generation can finish.
    fn alloc(&mut self, span: Span) -> u8 {
        if self.free_reg >= MAX_REGS {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                self.diags.push(
                    Diagnostic::new(
                        "E0301",
                        "expression is too complex: more than 255 registers needed",
                        span,
                    )
                    .help("split it into smaller expressions"),
                );
            }
            return (MAX_REGS - 1) as u8;
        }
        let r = self.free_reg as u8;
        self.free_reg += 1;
        self.max_reg = self.max_reg.max(self.free_reg);
        r
    }

    /// Release `reg` and every register above it.
    fn free_to(&mut self, reg: u8) {
        self.free_reg = self.free_reg.min(reg as u16);
    }

    /// Index of a constant in the pool, adding it if new.
    fn constant(&mut self, k: Const, span: Span) -> u16 {
        let key = match k {
            Const::Int(n) => ConstKey::Int(n),
            Const::Float(x) => ConstKey::Float(x.to_bits()),
        };
        if let Some(&i) = self.consts.get(&key) {
            return i;
        }
        let Ok(i) = u16::try_from(self.proto.consts.len()) else {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                self.diags.push(Diagnostic::new(
                    "E0301",
                    "too many constants in one function (max 65536)",
                    span,
                ));
            }
            return 0;
        };
        self.proto.consts.push(k);
        self.consts.insert(key, i);
        i
    }

    /// Add a nested prototype; returns its `CLOSURE` index.
    fn child(&mut self, p: Proto, span: Span) -> u16 {
        self.proto.protos.push(Rc::new(p));
        u16::try_from(self.proto.protos.len() - 1).unwrap_or_else(|_| {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                self.diags.push(Diagnostic::new(
                    "E0301",
                    "too many functions (max 65536)",
                    span,
                ));
            }
            0
        })
    }

    /// A global's index as a 16-bit `Bx` operand (E0301 if too many).
    fn global_index(&mut self, g: GlobalId, span: Span) -> u16 {
        u16::try_from(g.0).unwrap_or_else(|_| {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                self.diags.push(Diagnostic::new(
                    "E0301",
                    "too many top-level bindings (max 65536)",
                    span,
                ));
            }
            0
        })
    }

    /// Load the value a name refers to into `dst`.
    fn name(&mut self, e: &Expr, dst: u8) {
        match self.resolved.names.get(&e.id).copied() {
            Some(Res::Global(g)) => {
                let bx = self.global_index(g, e.span);
                self.emit(Instr::abx(OpCode::GETGLOBAL, dst, bx), e.span);
            }
            Some(Res::Upvalue { index, .. }) => {
                self.emit(Instr::abc(OpCode::GETUPVAL, dst, index, 0), e.span);
            }
            Some(Res::SelfRef(_)) => self.emit(Instr::abc(OpCode::GETSELF, dst, 0, 0), e.span),
            Some(Res::Local(b)) => {
                let r = self.locals[&b];
                // A local already in `dst` needs no copy.
                if r != dst {
                    self.emit(Instr::abc(OpCode::MOVE, dst, r, 0), e.span);
                }
            }
            None => unreachable!("resolver left a name unresolved"),
        }
    }

    /// Load an integer: inline (`LOADI`) if it fits in 16 bits, otherwise via
    /// the constant pool (`LOADK`).
    fn load_int(&mut self, n: i64, dst: u8, span: Span) {
        match i16::try_from(n) {
            Ok(i) => self.emit(Instr::asbx(OpCode::LOADI, dst, i), span),
            Err(_) => {
                let k = self.constant(Const::Int(n), span);
                self.emit(Instr::abx(OpCode::LOADK, dst, k), span);
            }
        }
    }

    /// Load a float (always from the constant pool).
    fn load_float(&mut self, x: f64, dst: u8, span: Span) {
        let k = self.constant(Const::Float(x), span);
        self.emit(Instr::abx(OpCode::LOADK, dst, k), span);
    }

    /// Emit a `JMP` with a placeholder offset; returns its index for
    /// `patch_here` ("backpatching": the target is not known yet).
    fn emit_jmp(&mut self, span: Span) -> usize {
        self.emit(Instr::sj(OpCode::JMP, 0), span);
        self.proto.code.len() - 1
    }

    /// Point the `JMP` at index `at` to the next instruction to be emitted.
    fn patch_here(&mut self, at: usize) {
        // Offsets are relative to the instruction after the jump.
        let offset = self.proto.code.len() as i64 - (at as i64 + 1);
        if !(i64::from(MIN_SJ)..=i64::from(MAX_SJ)).contains(&offset) {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                let s = self.proto.spans[at];
                let span = Span {
                    start: s.start,
                    end: s.end,
                };
                self.diags.push(Diagnostic::new(
                    "E0301",
                    "jump too far: a branch spans more than 8 million instructions",
                    span,
                ));
            }
            return;
        }
        self.proto.code[at] = Instr::sj(OpCode::JMP, offset as i32);
    }

    /// Point the `JMPIFSET` at index `at` to the next instruction (16-bit
    /// offset; E0301 if a default expression is too long).
    fn patch_sbx(&mut self, at: usize) {
        let offset = self.proto.code.len() - (at + 1);
        let Ok(off) = i16::try_from(offset) else {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                let s = self.proto.spans[at];
                self.diags.push(Diagnostic::new(
                    "E0301",
                    "default value too long: more than 32767 instructions",
                    Span {
                        start: s.start,
                        end: s.end,
                    },
                ));
            }
            return;
        };
        let a = self.proto.code[at].a();
        self.proto.code[at] = Instr::asbx(OpCode::JMPIFSET, a, off);
    }

    /// Evaluate `e` into `dst`; `tail` = `e` is in tail position (its value
    /// is the function's result, M5). Tail positions: the fn body, both
    /// branches of an `if` in tail position, the last line of a block in
    /// tail position. A call there is compiled as `TAILCALL`. Operands of
    /// `and`/`or`, comparisons and anything in `<main>` are never tail
    /// positions.
    fn expr_at(&mut self, e: &Expr, dst: u8, tail: bool) {
        if !tail {
            return self.expr(e, dst);
        }
        match &e.kind {
            ExprKind::Call {
                callee,
                args,
                piped,
            } => self.call(e, callee, args, *piped, dst, true),
            ExprKind::If {
                cond,
                then_br,
                else_br,
            } => self.if_expr(e, cond, then_br, else_br, dst, true),
            ExprKind::Block(items) => self.block(items, dst, true),
            _ => self.expr(e, dst),
        }
    }

    /// Evaluate `e` into register `dst`.
    /// EXTEND: every new `ExprKind` variant needs an arm here.
    fn expr(&mut self, e: &Expr, dst: u8) {
        match &e.kind {
            ExprKind::Int(n) => self.load_int(*n, dst, e.span),
            ExprKind::Float(x) => self.load_float(*x, dst, e.span),
            ExprKind::Bool(b) => {
                self.emit(Instr::abc(OpCode::LOADBOOL, dst, u8::from(*b), 0), e.span)
            }
            ExprKind::Unit => self.emit(Instr::abc(OpCode::LOADUNIT, dst, 0, 0), e.span),
            ExprKind::Call {
                callee,
                args,
                piped,
            } => self.call(e, callee, args, *piped, dst, false),
            ExprKind::Pipe { .. } => unreachable!("pipes are removed by desugar.rs"),
            ExprKind::Name(_) => self.name(e, dst),
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            } => {
                // Fold `-<literal>`: same value, no NEG at runtime.
                // (`-n` cannot overflow: the lexer only produces 0..=i64::MAX.)
                match operand.kind {
                    ExprKind::Int(n) => return self.load_int(-n, dst, e.span),
                    ExprKind::Float(x) => return self.load_float(-x, dst, e.span),
                    _ => {}
                }
                self.expr(operand, dst);
                self.emit(Instr::abc(OpCode::NEG, dst, dst, 0), e.span);
            }
            ExprKind::Not(operand) => {
                self.expr(operand, dst);
                self.emit(Instr::abc(OpCode::NOT, dst, dst, 0), e.span);
            }
            ExprKind::Binary { op, lhs, rhs, .. } => {
                let opcode = match op {
                    BinOp::Add => OpCode::ADD,
                    BinOp::Sub => OpCode::SUB,
                    BinOp::Mul => OpCode::MUL,
                    BinOp::Div => OpCode::DIV,
                    BinOp::IDiv => OpCode::IDIV,
                    BinOp::Mod => OpCode::MOD,
                    BinOp::Pow => OpCode::POW,
                };
                // Left operand straight into dst, right operand into a fresh
                // temporary, combine into dst, release the temporary.
                self.expr(lhs, dst);
                let tmp = self.alloc(rhs.span);
                self.expr(rhs, tmp);
                self.emit(Instr::abc(opcode, dst, dst, tmp), e.span);
                self.free_to(tmp);
            }
            ExprKind::Logic {
                op,
                op_span,
                lhs,
                rhs,
            } => self.logic(*op, *op_span, lhs, rhs, dst),
            ExprKind::Compare { first, rest } => self.compare(first, rest, dst),
            ExprKind::If {
                cond,
                then_br,
                else_br,
            } => self.if_expr(e, cond, then_br, else_br, dst, false),
            ExprKind::Block(items) => self.block(items, dst, false),
            ExprKind::Assert(inner) => self.assert(inner, dst),
            ExprKind::Lambda(l) => self.lambda(e, l, None, dst),
        }
    }

    /// The initialiser of `let b`: a lambda written there is named after
    /// the binding (plan D23).
    fn let_init(&mut self, b: &Binding, dst: u8) {
        match &b.init.kind {
            ExprKind::Lambda(l) if l.partial_of.is_none() => {
                self.lambda(&b.init, l, Some(&b.name), dst)
            }
            _ => self.expr(&b.init, dst),
        }
    }

    /// A lambda: compile its body as a function, then `CLOSURE` into `dst`.
    /// Display name (D23): the binding it is assigned to, `<partial f@l:c>`
    /// for a `_` partial, else `<lambda@l:c>`.
    fn lambda(&mut self, e: &Expr, l: &Lambda, binding: Option<&str>, dst: u8) {
        let (line, col) = self.sm.line_col(e.span.start);
        let name = match (&l.partial_of, binding) {
            (Some(f), _) => format!("<partial {f}@{line}:{col}>"),
            (None, Some(b)) => b.to_string(),
            (None, None) => format!("<lambda@{line}:{col}>"),
        };
        match compile_function(&name, e.span, &l.params, &l.body, self.resolved, self.sm) {
            Ok(p) => self.closure(e.id, p, e.span, dst),
            Err(ds) => self.diags.extend(ds),
        }
    }

    /// Add `p` as a nested prototype and emit `CLOSURE dst k`. Fills in
    /// `p.upvals` from the resolver's capture list for node `id`: where this
    /// function finds each captured value (plan D5).
    fn closure(&mut self, id: NodeId, mut p: Proto, span: Span, dst: u8) {
        if let Some(caps) = self.resolved.captures.get(&id) {
            for c in &caps.list {
                let src = match c.from {
                    CaptureFrom::Local(b) => UpvalSrc::Reg(self.locals[&b]),
                    CaptureFrom::Upvalue(i) => UpvalSrc::Upval(i),
                    CaptureFrom::SelfFn => UpvalSrc::Current,
                };
                p.upvals.push(UpvalDesc {
                    src,
                    name: self.resolved.locals[c.binding.0 as usize]
                        .name
                        .as_str()
                        .into(),
                });
            }
        }
        let k = self.child(p, span);
        self.emit(Instr::abx(OpCode::CLOSURE, dst, k), span);
    }

    /// A call (see the module docs). The window `[base, base+n]` is reserved
    /// before anything is evaluated, so the arguments can be computed in any
    /// order straight into their slots: a piped argument first (Q17), then
    /// the callee, then the rest in source order. With a `CallPlan` (known
    /// callee, M4b) slot `k` is parameter `k`: keyword arguments land in
    /// their parameter's slot and omitted parameters get `LOADMISSING`.
    fn call(
        &mut self,
        e: &Expr,
        callee: &Expr,
        args: &[Arg],
        piped: Option<usize>,
        dst: u8,
        tail: bool,
    ) {
        // Reuse dst as the window base when it is the topmost register.
        let base = if u16::from(dst) + 1 == self.free_reg {
            dst
        } else {
            self.alloc(callee.span)
        };
        // reg_of[i] = register of args[i]; `missing` = slots with no argument.
        let mut reg_of: Vec<Option<u8>> = vec![None; args.len()];
        let mut missing = Vec::new();
        let nslots = match self.resolved.calls.get(&e.id) {
            Some(plan) => {
                for slot in &plan.slots {
                    let r = self.alloc(e.span);
                    match *slot {
                        Slot::Arg(i) => reg_of[i] = Some(r),
                        Slot::Missing => missing.push(r),
                    }
                }
                plan.slots.len()
            }
            None => {
                for (i, a) in args.iter().enumerate() {
                    reg_of[i] = Some(self.alloc(arg_span(a)));
                }
                args.len()
            }
        };
        let piped = piped.filter(|&k| k < args.len());
        if let Some(k) = piped {
            if let (Some(x), Some(r)) = (arg_value(&args[k]), reg_of[k]) {
                self.expr(x, r);
            }
        }
        self.expr(callee, base);
        for (i, (a, r)) in args.iter().zip(&reg_of).enumerate() {
            if Some(i) == piped {
                continue;
            }
            if let (Some(x), Some(r)) = (arg_value(a), *r) {
                self.expr(x, r);
            }
        }
        for r in missing {
            self.emit(Instr::abc(OpCode::LOADMISSING, r, 0, 0), e.span);
        }
        // In tail position the callee replaces this frame and returns to our
        // caller itself (M5); nothing after the TAILCALL runs.
        let op = if tail { OpCode::TAILCALL } else { OpCode::CALL };
        self.emit(Instr::abc(op, base, nslots.min(255) as u8, 0), e.span);
        if base != dst {
            // After a TAILCALL nothing runs: no copy to dst.
            if !tail {
                self.emit(Instr::abc(OpCode::MOVE, dst, base, 0), e.span);
            }
            self.free_to(base);
        } else {
            self.free_to(base + 1);
        }
    }

    /// Indented block: each `let` gets the next free register for the rest
    /// of the block; the last expression goes into `dst`; then every
    /// register the block allocated is released, so the next block reuses
    /// them.
    fn block(&mut self, items: &[Item], dst: u8, tail: bool) {
        let mark = self.free_reg;
        let first_local = self.proto.locals.len();
        for (i, item) in items.iter().enumerate() {
            let last = i + 1 == items.len();
            match item {
                Item::Expr(e) if last => self.expr_at(e, dst, tail),
                Item::Expr(e) => {
                    let tmp = self.alloc(e.span);
                    self.expr(e, tmp);
                    self.free_to(tmp);
                }
                // A local fn (S21): a closure in the next register, bound
                // like a `let` for the rest of the block.
                Item::Fn(d) => {
                    let r = self.alloc(d.name_span);
                    match compile_function(
                        &d.name,
                        d.name_span,
                        &d.params,
                        &d.body,
                        self.resolved,
                        self.sm,
                    ) {
                        Ok(p) => self.closure(d.id, p, d.name_span, r),
                        Err(ds) => self.diags.extend(ds),
                    }
                    match self.resolved.lets.get(&d.id).copied().flatten() {
                        Some(Res::Local(id)) => self.declare_local(id, r),
                        _ => self.free_to(r),
                    }
                }
                Item::Let(b) => {
                    let r = self.alloc(b.init.span);
                    self.let_init(b, r);
                    match self.resolved.lets.get(&b.id).copied().flatten() {
                        Some(Res::Local(id)) => self.declare_local(id, r),
                        // `let _ = …`: evaluated, then dropped.
                        _ => self.free_to(r),
                    }
                }
            }
        }
        // The block's `let`s go out of scope here (debug info, D3).
        self.close_locals(first_local);
        self.free_to(mark as u8);
    }

    /// ```text
    ///       cond → dst
    ///       TEST dst 0        ; skip the JMP if cond is true
    ///       JMP  else
    ///       then → dst
    ///       JMP  end
    /// else: else → dst
    /// end:
    /// ```
    fn if_expr(
        &mut self,
        e: &Expr,
        cond: &Expr,
        then_br: &Expr,
        else_br: &Expr,
        dst: u8,
        tail: bool,
    ) {
        // The condition can use `dst`: the branches overwrite it anyway.
        self.expr(cond, dst);
        self.emit(Instr::abc(OpCode::TEST, dst, 0, 0), cond.span);
        let to_else = self.emit_jmp(cond.span);
        self.expr_at(then_br, dst, tail);
        let to_end = self.emit_jmp(e.span);
        self.patch_here(to_else);
        self.expr_at(else_br, dst, tail);
        self.patch_here(to_end);
    }

    /// Short circuit: `a and b` = if a then b else false; `a or b` = if a
    /// then true else b. The result is the last operand evaluated.
    ///
    /// ```text
    ///      lhs → dst
    ///      TEST dst 0|1     ; and: jump when false; or: jump when true
    ///      JMP  end
    ///      rhs → dst
    ///      [TEST dst 0; JMP +0]   only if rhs is not syntactically a bool
    /// end:
    /// ```
    fn logic(&mut self, op: LogicOp, op_span: Span, lhs: &Expr, rhs: &Expr, dst: u8) {
        self.expr(lhs, dst);
        let jump_when = u8::from(op == LogicOp::Or);
        self.emit(Instr::abc(OpCode::TEST, dst, jump_when, 0), lhs.span);
        let to_end = self.emit_jmp(op_span);
        self.expr(rhs, dst);
        if !is_bool_syntax(rhs) {
            // Operands must be bools (syntax.md S11). TEST checks the type;
            // both of its outcomes continue at the same place.
            self.emit(Instr::abc(OpCode::TEST, dst, 0, 0), rhs.span);
            self.emit(Instr::sj(OpCode::JMP, 0), rhs.span);
        }
        self.patch_here(to_end);
    }

    /// Comparison chain `a op1 b op2 c …` (Python): each operand evaluated
    /// once, in order; stop at the first false link.
    ///
    /// ```text
    ///      a → dst;  b → t1
    ///      op1 dst dst t1
    ///      TEST dst 0; JMP end
    ///      c → t2
    ///      op2 dst t1 t2
    /// end:
    /// ```
    fn compare(&mut self, first: &Expr, rest: &[CmpLink], dst: u8) {
        self.expr(first, dst);
        let mark = self.free_reg;
        let mut lhs_reg = dst;
        let mut lhs_span = first.span;
        let mut to_end = Vec::new();
        for (i, link) in rest.iter().enumerate() {
            let r = self.alloc(link.rhs.span);
            self.expr(&link.rhs, r);
            self.emit_cmp(link.op, dst, lhs_reg, r, lhs_span.to(link.rhs.span));
            if i + 1 < rest.len() {
                self.emit(Instr::abc(OpCode::TEST, dst, 0, 0), link.op_span);
                to_end.push(self.emit_jmp(link.op_span));
            }
            lhs_reg = r;
            lhs_span = link.rhs.span;
        }
        self.free_to(mark as u8);
        for at in to_end {
            self.patch_here(at);
        }
    }

    /// `R[dst] = R[l] op R[r]`; `>`/`>=` are `<`/`<=` with swapped operands (D18).
    fn emit_cmp(&mut self, op: CmpOp, dst: u8, l: u8, r: u8, span: Span) {
        let (opcode, b, c) = match op {
            CmpOp::Eq => (OpCode::EQ, l, r),
            CmpOp::Ne => (OpCode::NE, l, r),
            CmpOp::Lt => (OpCode::LT, l, r),
            CmpOp::Le => (OpCode::LE, l, r),
            CmpOp::Gt => (OpCode::LT, r, l),
            CmpOp::Ge => (OpCode::LE, r, l),
        };
        self.emit(Instr::abc(opcode, dst, b, c), span);
    }

    /// `assert e` (D29). For a single comparison `a op b`, both sides are
    /// kept in the two registers after the result, so a failure can print
    /// `left:` and `right:` (`ASSERT t 1`). The `ASSERT` instruction's span
    /// is `e`, which the error renderer quotes. Value: `()`.
    fn assert(&mut self, inner: &Expr, dst: u8) {
        // Reuse dst as the result register when it is the topmost one, so
        // t, t+1, t+2 are consecutive.
        let t = if u16::from(dst) + 1 == self.free_reg {
            dst
        } else {
            self.alloc(inner.span)
        };
        match &inner.kind {
            ExprKind::Compare { first, rest } if rest.len() == 1 => {
                let l = self.alloc(first.span);
                let r = self.alloc(rest[0].rhs.span);
                self.expr(first, l);
                self.expr(&rest[0].rhs, r);
                self.emit_cmp(rest[0].op, t, l, r, inner.span);
                self.emit(Instr::abc(OpCode::ASSERT, t, 1, 0), inner.span);
                self.free_to(l);
            }
            _ => {
                self.expr(inner, t);
                self.emit(Instr::abc(OpCode::ASSERT, t, 0, 0), inner.span);
            }
        }
        if t != dst {
            self.free_to(t);
        }
        self.emit(Instr::abc(OpCode::LOADUNIT, dst, 0, 0), inner.span);
    }
}

/// The expression of a call argument (`None` for `_`, which `desugar.rs`
/// removes before code generation).
fn arg_value(a: &Arg) -> Option<&Expr> {
    match a {
        Arg::Positional(x) | Arg::Keyword { value: x, .. } => Some(x),
        Arg::Placeholder(_) => None,
    }
}

/// Source span of a call argument.
fn arg_span(a: &Arg) -> Span {
    match a {
        Arg::Positional(x) | Arg::Keyword { value: x, .. } => x.span,
        Arg::Placeholder(s) => *s,
    }
}

/// Whether `e` produces a bool by construction (no runtime check needed when
/// it is the right operand of `and`/`or`).
fn is_bool_syntax(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Bool(_) | ExprKind::Not(_) | ExprKind::Compare { .. } | ExprKind::Logic { .. }
    )
}

// Tests: `cargo test -p valo-compiler codegen`.
#[cfg(test)]
mod tests {
    use crate::span::SourceMap;
    use crate::{CompileOptions, compile_source};

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__toplevel_globals.snap
    #[test]
    fn toplevel_globals() {
        insta::assert_snapshot!(disasm("let a = 1\nlet b = a + 2\nlet _ = b\nb * a"));
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__nested_conditional.snap
    #[test]
    fn nested_conditional() {
        insta::assert_snapshot!(disasm(
            "let n = 5\nif n < 0: -1\nelif n == 0: 0\nelse: 1\nif n > 2 then 1.5 else 2.5"
        ));
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__block_registers_are_reused.snap
    #[test]
    fn block_registers_are_reused() {
        // Hand count: the `if` value goes to R0 (dst). First block: a → R1,
        // b → R2, `a + b` uses R0 and temp R3 → 4 registers. Second block:
        // c → R1 again (the first block's registers were released) → no more.
        let d = disasm(
            "if true:\n    let a = 1\n    let b = 2\n    a + b\nelse: 0\n\
             if true:\n    let c = 3\n    c\nelse: 0",
        );
        assert!(d.starts_with("function <main> arity=0 regs=4 "), "{d}");
        insta::assert_snapshot!(d);
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__chain_logic_assert.snap
    #[test]
    fn chain_logic_assert() {
        insta::assert_snapshot!(disasm(
            "let x = 3\n0 < x <= 10\nx > 1 and x\nx > 1 or false\nassert x * 2 == 6\nassert not false"
        ));
    }

    #[test]
    fn float_constants_keep_signed_zero() {
        let d = disasm("0.0 + -0.0 + 0.0 + 1.5");
        assert!(
            d.contains("constants:\n  0  float 0.0\n  1  float -0.0\n  2  float 1.5\n"),
            "{d}"
        );
    }

    fn disasm(src: &str) -> String {
        let sm = SourceMap::new("t.vl", src);
        let p = compile_source(&sm, CompileOptions::default()).expect("compiles");
        valo_vm::disasm::disassemble(&p)
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__three_op_expression.snap
    #[test]
    fn three_op_expression() {
        insta::assert_snapshot!(disasm("(1 + 70000) * -3 - 2 ** 2"));
    }

    #[test]
    fn constants_are_deduplicated() {
        let d = disasm("100000 + 100000 + 40000");
        assert!(
            d.contains("constants:\n  0  int 100000\n  1  int 40000\n"),
            "{d}"
        );
    }

    #[test]
    fn register_limit_is_reported() {
        // A comparison chain keeps one register per operand (no recursion).
        let operands: Vec<String> = (0..300).map(|i| i.to_string()).collect();
        let sm = SourceMap::new("t.vl", operands.join(" < "));
        let err = compile_source(&sm, CompileOptions::default()).unwrap_err();
        assert_eq!(err.iter().map(|d| d.code).collect::<Vec<_>>(), ["E0301"]);
    }

    #[test]
    fn deep_nesting_compiles() {
        // The deepest expression the parser accepts goes through every pass
        // on a test thread's small stack.
        let n = crate::parser::MAX_DEPTH as usize / 2 - 1;
        let src = format!("{}1{}", "(1 + ".repeat(n), ")".repeat(n));
        let sm = SourceMap::new("t.vl", src);
        assert!(compile_source(&sm, CompileOptions::default()).is_ok());
        let ifs = format!(
            "{}1{}",
            "if true then ".repeat(n / 2),
            " else 0".repeat(n / 2)
        );
        let sm = SourceMap::new("t.vl", ifs);
        assert!(compile_source(&sm, CompileOptions::default()).is_ok());
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__two_functions.snap
    #[test]
    fn two_functions() {
        insta::assert_snapshot!(disasm(
            "print(sq(3) + add(1, sq(2)))\nfn sq(x: int) -> int = x * x\n\
             fn add(a: int, b: int) -> int:\n    let s = a + b\n    s\n\
             let r = 5 |> add(1)\nr |> print"
        ));
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__defaults_prologue.snap
    #[test]
    fn defaults_prologue() {
        // Two defaults (the second uses the first parameter); calls with
        // keywords out of order, with a gap, and through a pipe.
        insta::assert_snapshot!(disasm(
            "fn f(a: int, b: int = 1, c: int = a * 2) -> int = a + b + c\n\
             f(1, c=3, b=4)\nf(1, c=3)\nf(1)\n5 |> f(b=0)"
        ));
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__closures.snap
    #[test]
    fn closures() {
        // `mid` captures `a` from a register of `outer`; the lambda captures
        // `a` from `mid`'s captures, `b` from a register and `mid` itself.
        insta::assert_snapshot!(disasm(
            "fn outer(a: int) -> int:\n    fn mid(b: int) -> int:\n        \
             let f = c => a + b + c + mid(0)\n        f(1)\n    mid(10)"
        ));
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__partials.snap
    #[test]
    fn partials() {
        // A partial binds its fixed argument once, then builds the lambda; a
        // pipe into `_` is a plain call (no CLOSURE).
        insta::assert_snapshot!(disasm(
            "fn add(a: int, b: int) -> int = a + b\nlet inc = add(1, _)\nprint(5 |> add(2, _))"
        ));
    }

    // Snapshot: src/snapshots/valo_compiler__codegen__tests__tail_calls.snap
    #[test]
    fn tail_calls() {
        // Tail calls in both branches and at the end of a block; the call in
        // `1 + loop(...)` and the calls in <main> are not tail calls.
        insta::assert_snapshot!(disasm(
            "fn loop(n: int, acc: int) -> int:\n    if n == 0: acc\n    elif n < 0: loop(-n, acc)\n    \
             else:\n        let m = n - 1\n        loop(m, acc + 1)\n\
             fn g(n: int) -> int = 1 + loop(n, 0)\nprint(g(3))"
        ));
    }
}
