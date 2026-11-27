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
//! | compile a new top-level item (`fn`, `def`)  | `compile`, the `for item in &program.items` loop  |
//! | change how names are loaded                 | `FuncState::name` (globals: `GETGLOBAL`, locals: `MOVE`) |
//! | map an operator to an opcode                | `ExprKind::Binary` arm (`let opcode = match op`) |
//! | add a constant type (float, string)         | a dedup map like `int_consts` + `Const` in `vm/src/chunk.rs` |
//! | add an optimisation (constant folding…)     | `FuncState::expr` (see the `-<literal>` fold)    |
//! | change register limits                      | `MAX_REGS`, `FuncState::alloc`                   |
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

use valo_vm::chunk::{Const, GlobalDesc, Program, Proto, SrcSpan};
use valo_vm::opcode::{Instr, OpCode};

use crate::ast::{self, BinOp, Expr, ExprKind, Item, UnaryOp};
use crate::diagnostic::Diagnostic;
use crate::resolve::{BindingId, GlobalId, Res, Resolved};
use crate::span::{SourceMap, Span};

/// Code generation options.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Emit `ECHO` after every top-level expression (`valo run --echo`).
    pub echo: bool,
}

/// Compile a parsed and resolved program into the `<main>` prototype.
///
/// Every top-level expression is evaluated into a temporary (and echoed with
/// `--echo`); every top-level `let` stores its value into a global slot
/// (`SETGLOBAL`, plan D4). The program ends with `LOADUNIT` + `RETURN`.
pub fn compile(
    program: &ast::Program,
    resolved: &Resolved,
    sm: &SourceMap,
    opts: Options,
) -> Result<Program, Vec<Diagnostic>> {
    let mut fs = FuncState::new("<main>", Span::new(0, 0), resolved);
    // EXTEND: new `Item` kinds (fn, def, ...) get arms here.
    for item in &program.items {
        match item {
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
                fs.expr(&b.init, tmp);
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
    let main = fs.finish()?;
    Ok(Program {
        main: Rc::new(main),
        globals: resolved
            .globals
            .iter()
            .map(|g| GlobalDesc {
                name: g.name.as_str().into(),
            })
            .collect(),
        source_name: sm.name.as_str().into(),
        line_starts: sm.line_starts.clone(),
    })
}

/// Per-function compilation state (Lua `FuncState`). One per function being
/// compiled; M4 will keep a stack of them for nested functions.
// RUST: `'r` ties the borrowed resolver output to the FuncState's lifetime.
struct FuncState<'r> {
    /// Name resolution results (which slot each name refers to).
    resolved: &'r Resolved,
    /// Register holding each local binding currently in scope.
    locals: HashMap<BindingId, u8>,
    /// The prototype being filled in.
    proto: Proto,
    /// First free register.
    free_reg: u16,
    /// High-water mark of `free_reg` = number of registers the frame needs.
    max_reg: u16,
    /// Value → index in `proto.consts`, so each constant is stored once.
    int_consts: HashMap<i64, u16>,
    /// Errors found while generating code.
    diags: Vec<Diagnostic>,
}

/// Registers are 8-bit operands.
const MAX_REGS: u16 = 255;

impl<'r> FuncState<'r> {
    fn new(name: &str, def_span: Span, resolved: &'r Resolved) -> FuncState<'r> {
        FuncState {
            resolved,
            locals: HashMap::new(),
            proto: Proto {
                name: name.into(),
                arity: 0,
                num_regs: 0,
                code: Vec::new(),
                consts: Vec::new(),
                spans: Vec::new(),
                def_span: def_span.into(),
            },
            free_reg: 0,
            max_reg: 0,
            int_consts: HashMap::new(),
            diags: Vec::new(),
        }
    }

    /// Finish the prototype, or return the errors collected on the way.
    // RUST: `mut self` (no `&`) consumes the FuncState; it cannot be used
    // after `finish`, which is exactly right.
    fn finish(mut self) -> Result<Proto, Vec<Diagnostic>> {
        if !self.diags.is_empty() {
            return Err(self.diags);
        }
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

    /// Index of `n` in the constant pool, adding it if new.
    fn int_const(&mut self, n: i64, span: Span) -> u16 {
        if let Some(&k) = self.int_consts.get(&n) {
            return k;
        }
        let Ok(k) = u16::try_from(self.proto.consts.len()) else {
            if !self.diags.iter().any(|d| d.code == "E0301") {
                self.diags.push(Diagnostic::new(
                    "E0301",
                    "too many constants in one function (max 65536)",
                    span,
                ));
            }
            return 0;
        };
        self.proto.consts.push(Const::Int(n));
        self.int_consts.insert(n, k);
        k
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
                let k = self.int_const(n, span);
                self.emit(Instr::abx(OpCode::LOADK, dst, k), span);
            }
        }
    }

    /// Evaluate `e` into register `dst`.
    /// EXTEND: every new `ExprKind` variant needs an arm here.
    fn expr(&mut self, e: &Expr, dst: u8) {
        match &e.kind {
            ExprKind::Int(n) => self.load_int(*n, dst, e.span),
            ExprKind::Name(_) => self.name(e, dst),
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            } => {
                // Fold `-<literal>`: same value, no NEG at runtime.
                // (`-n` cannot overflow: the lexer only produces 0..=i64::MAX.)
                if let ExprKind::Int(n) = operand.kind {
                    return self.load_int(-n, dst, e.span);
                }
                self.expr(operand, dst);
                self.emit(Instr::abc(OpCode::NEG, dst, dst, 0), e.span);
            }
            ExprKind::Binary {
                op,
                op_span,
                lhs,
                rhs,
            } => {
                let opcode = match op {
                    BinOp::Add => OpCode::ADD,
                    BinOp::Sub => OpCode::SUB,
                    BinOp::Mul => OpCode::MUL,
                    BinOp::IDiv => OpCode::IDIV,
                    BinOp::Mod => OpCode::MOD,
                    BinOp::Pow => OpCode::POW,
                    // EXTEND (M3): `/` becomes OpCode::DIV once floats exist.
                    BinOp::Div => {
                        self.diags.push(
                            Diagnostic::new(
                                "E0009",
                                "true division `/` is not supported yet",
                                *op_span,
                            )
                            .label("`/` always produces a float; floats are not implemented yet")
                            .help("use `//` for integer floor division"),
                        );
                        return;
                    }
                };
                // Left operand straight into dst, right operand into a fresh
                // temporary, combine into dst, release the temporary.
                self.expr(lhs, dst);
                let tmp = self.alloc(rhs.span);
                self.expr(rhs, tmp);
                self.emit(Instr::abc(opcode, dst, dst, tmp), e.span);
                self.free_to(tmp);
            }
        }
    }
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
        let deep = format!("{}1{}", "(1 + ".repeat(300), ")".repeat(300));
        let sm = SourceMap::new("t.vl", deep);
        let err = compile_source(&sm, CompileOptions::default()).unwrap_err();
        assert_eq!(err.iter().map(|d| d.code).collect::<Vec<_>>(), ["E0301"]);
    }
}
