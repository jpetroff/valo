//! # Disassembler (`disasm.rs`)
//!
//! **What this file is:** turns a compiled [`Program`] back into readable
//! text, one instruction per line. It is the main debugging tool for the
//! compiler: `valo check file.vl --dump bytecode` prints it, and snapshot
//! tests compare it.
//!
//! ```text
//! function <main> arity=0 regs=2 consts=1 protos=0   ; file.vl:1:1
//!   0000  [1:1]   LOADI     0 2
//!   0001  [1:5]   LOADK     1 0        ; 70000
//!   0002  [1:1]   ADD       0 0 1
//! constants:
//!   0  int 70000
//! ```
//!
//! A function with parameters or `let`s also lists its debug table:
//! `locals:` with one `R<reg> <name> pc <start>..<end>` line each.
//!
//! Columns: instruction index (`pc`), `[line:col]` of the source that produced
//! it, mnemonic, operands (`A B C`, `A Bx`, …), and an optional `; comment`
//! (the constant value for `LOADK`, the global's name for `GETGLOBAL` /
//! `SETGLOBAL`, the target for `JMP`).
//!
//! **Textbook chapter:** `docs/05-bytecode-format.md` (section *Reading a
//! disassembly*).
//!
//! ## Where to change things
//!
//! | I want to…                           | Look at                         |
//! |--------------------------------------|---------------------------------|
//! | show a helpful comment for an opcode | `let comment = match op { ... }`|
//! | print a new kind of constant         | `const_repr`, `const_entry`     |
//! | change how nested functions print    | end of `proto` (children follow the parent's constants, indented one level) |
//! | change the `locals:` debug table     | after `constants:` in `proto`   |
//!
//! Changing the format changes snapshot files
//! (`compiler/src/snapshots/*.snap`); re-run tests with `INSTA_UPDATE=always`
//! and review the diff (see `docs/10-testing.md`).
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `writeln!(out, ...)` into a `String` (needs `use std::fmt::Write`) (*Macros*).
//! - format specifiers: `{pc:04}` zero-padded width 4, `{pos:<7}` left-aligned width 7.
//! - `let Ok(op) = ... else { ...; continue; }` — "let-else" (*Pattern matching*).
//! - `iter().zip(..).enumerate()` (*Closures and iterators*).

use std::fmt::Write;

use crate::chunk::{Const, Program, Proto, UpvalSrc};
use crate::opcode::{Format, OpCode};

/// Disassemble a whole program.
pub fn disassemble(program: &Program) -> String {
    let mut out = String::new();
    proto(&mut out, program, &program.main, 0);
    out
}

/// Disassemble one prototype; `depth` indents nested functions (from M4).
fn proto(out: &mut String, program: &Program, p: &Proto, depth: usize) {
    let pad = "  ".repeat(depth);
    let (dl, dc) = program.line_col(p.def_span.start);
    // RUST: writing into a String cannot fail, but `writeln!` still returns a
    // `Result`; `let _ =` explicitly ignores it.
    // `required=` only when some parameters have defaults (M4b).
    let required = if p.required == p.arity {
        String::new()
    } else {
        format!(" required={}", p.required)
    };
    // `upvals=` only for closures that capture something (M6).
    let upvals = if p.upvals.is_empty() {
        String::new()
    } else {
        format!(" upvals={}", p.upvals.len())
    };
    let _ = writeln!(
        out,
        "{pad}function {} arity={}{required} regs={} consts={}{upvals} protos={}   ; {}:{dl}:{dc}",
        p.name,
        p.arity,
        p.num_regs,
        p.consts.len(),
        p.protos.len(),
        program.source_name
    );
    // Walk instructions and their spans side by side.
    for (pc, (instr, span)) in p.code.iter().zip(&p.spans).enumerate() {
        let (l, c) = program.line_col(span.start);
        let pos = format!("[{l}:{c}]");
        let Ok(op) = instr.op() else {
            let _ = writeln!(
                out,
                "{pad}  {pc:04}  {pos:<7} <bad opcode {}>",
                instr.op_byte()
            );
            continue;
        };
        let operands = match op.format() {
            Format::A => format!("{}", instr.a()),
            Format::AB => format!("{} {}", instr.a(), instr.b()),
            Format::ABC => format!("{} {} {}", instr.a(), instr.b(), instr.c()),
            Format::ABx => format!("{} {}", instr.a(), instr.bx()),
            Format::AsBx => format!("{} {}", instr.a(), instr.sbx()),
            Format::SJ => format!("{}", instr.sj_offset()),
        };
        // EXTEND: add an arm here to annotate other opcodes.
        let comment = match op {
            OpCode::LOADK => p.consts.get(instr.bx() as usize).map(const_repr),
            OpCode::GETGLOBAL | OpCode::SETGLOBAL => program
                .globals
                .get(instr.bx() as usize)
                .map(|g| g.name.to_string()),
            OpCode::JMP => Some(format!("to {}", pc as i64 + 1 + instr.sj_offset() as i64)),
            OpCode::JMPIFSET => Some(format!(
                "to {} if passed",
                pc as i64 + 1 + instr.sbx() as i64
            )),
            OpCode::CLOSURE => p
                .protos
                .get(instr.bx() as usize)
                .map(|c| format!("<fn {}/{}>", c.name, c.arity)),
            OpCode::GETUPVAL => p.upvals.get(instr.b() as usize).map(|u| u.name.to_string()),
            OpCode::GETSELF => Some(p.name.to_string()),
            _ => None,
        };
        let line = format!("{pad}  {pc:04}  {pos:<7} {:<9} {operands:<8}", op.name());
        match comment {
            Some(c) => {
                let _ = writeln!(out, "{line} ; {c}");
            }
            None => {
                let _ = writeln!(out, "{}", line.trim_end());
            }
        }
    }
    if p.consts.is_empty() {
        let _ = writeln!(out, "{pad}constants: (none)");
    } else {
        let _ = writeln!(out, "{pad}constants:");
        for (i, k) in p.consts.iter().enumerate() {
            let _ = writeln!(out, "{pad}  {i}  {}", const_entry(k));
        }
    }
    // Captures (M6): where the enclosing function's CLOSURE takes each one.
    if !p.upvals.is_empty() {
        let _ = writeln!(out, "{pad}upvals:");
        for (i, u) in p.upvals.iter().enumerate() {
            let src = match u.src {
                UpvalSrc::Reg(r) => format!("R{r}"),
                UpvalSrc::Upval(j) => format!("upval {j}"),
                UpvalSrc::Current => "self".to_string(),
            };
            let _ = writeln!(out, "{pad}  {i:<3} {:<10} <- {src}", u.name);
        }
    }
    // Debug info for the debugger's Variables view (D3); only functions
    // with parameters or `let`s have any.
    if !p.locals.is_empty() {
        let _ = writeln!(out, "{pad}locals:");
        for l in &p.locals {
            let _ = writeln!(
                out,
                "{pad}  R{:<3} {:<10} pc {:04}..{:04}",
                l.reg, l.name, l.start_pc, l.end_pc
            );
        }
    }
    for child in &p.protos {
        proto(out, program, child, depth + 1);
    }
}

/// A constant as shown in a `; comment` after `LOADK`.
fn const_repr(k: &Const) -> String {
    match k {
        Const::Int(n) => n.to_string(),
        Const::Float(x) => format!("{x:?}"),
        Const::Str(t) => format!("{t:?}"),
    }
}

/// A constant as shown in the `constants:` table (with its type).
fn const_entry(k: &Const) -> String {
    match k {
        Const::Int(n) => format!("int {n}"),
        Const::Float(x) => format!("float {x:?}"),
        Const::Str(t) => format!("str {t:?}"),
    }
}
