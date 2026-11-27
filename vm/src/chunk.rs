//! # Compiled code containers (`chunk.rs`)
//!
//! **What this file is:** the data structures that hold compiled code.
//! A `Proto` (function *prototype*) is one compiled function: its
//! instructions, its constant pool and a table mapping each instruction back
//! to the source text. A `Program` is the whole compiled file: the `<main>`
//! prototype plus what is needed to print error locations.
//!
//! **Pipeline position:** output of `compiler/src/codegen.rs`, input of
//! `vm/src/vm.rs` and `vm/src/disasm.rs`. This module must never depend on
//! compiler types, because a later milestone (M11) writes `Program`s to
//! `.vlc` files and loads them without the compiler (plan §2.3).
//!
//! **Textbook chapter:** `docs/05-bytecode-format.md`.
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                      |
//! |----------------------------------------------|----------------------------------------------|
//! | add a new kind of constant (string, M7)      | `enum Const`, then `vm.rs` (`LOADK`), `disasm.rs` (`const_repr`, `const_entry`), `codegen.rs` |
//! | store more per-function data (params, upvalues) | `struct Proto` (+ `ProtoBuilder::new`, `FuncState::new` in codegen) |
//! | store more per-program data (types, M10)     | `struct Program` (+ `ProtoBuilder::program`, `codegen::compile`) |
//! | describe global slots (natives, M4)           | `struct GlobalDesc`, `enum GlobalKind`       |
//! | add nested functions                          | `Proto::protos` (indexed by `CLOSURE` Bx)    |
//! | build bytecode by hand in a VM test          | `ProtoBuilder` (see tests in `vm.rs`)       |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `Rc<Proto>` / `Rc<str>` — shared, reference-counted ownership (*Rc shared ownership*).
//! - `Vec<T>` — growable array (*Structs*).
//! - builder pattern: methods that take `self` and return `Self` so calls chain (*Ownership and borrowing*).
//! - `binary_search` returning `Ok(i)` / `Err(i)` (*Option and Result*).

use std::rc::Rc;

use crate::opcode::Instr;

/// Half-open byte range `[start, end)` into the program's source text.
///
/// The VM-side twin of `compiler::span::Span` (conversions live in
/// `compiler/src/span.rs`). Byte offsets, not line/column: line and column are
/// computed only when an error is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SrcSpan {
    /// First byte.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
}

impl SrcSpan {
    /// Construct a span.
    pub fn new(start: u32, end: u32) -> SrcSpan {
        // RUST: `SrcSpan { start, end }` is shorthand for
        // `SrcSpan { start: start, end: end }`.
        SrcSpan { start, end }
    }
}

/// A constant-pool entry.
///
/// Small integers are encoded directly in the instruction (`LOADI`); values
/// that do not fit go into the pool and are loaded with `LOADK index`.
/// EXTEND: strings (M7) become a new variant here.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    /// Integer too large for `LOADI`'s 16-bit immediate.
    Int(i64),
    /// Float literal (there is no float immediate form).
    Float(f64),
}

/// A compiled function.
///
/// RUST: `Rc<str>` is a shared, immutable string. Cloning it copies a pointer
/// and bumps a counter instead of copying the text, which is cheap for names
/// that are copied into every stack-trace entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Proto {
    /// Display name: `<main>`, a fn name, or `<lambda@l:c>`.
    pub name: Rc<str>,
    /// Number of parameters.
    pub arity: u8,
    /// Number of registers the frame needs.
    pub num_regs: u8,
    /// Instructions.
    pub code: Vec<Instr>,
    /// Constant pool (`LOADK` operands).
    pub consts: Vec<Const>,
    /// Source span of each instruction, parallel to `code`
    /// (`spans[pc]` belongs to `code[pc]`). Used for error locations.
    pub spans: Vec<SrcSpan>,
    /// Span of the whole definition.
    pub def_span: SrcSpan,
    /// Functions defined inside this one, in definition order. `CLOSURE A Bx`
    /// creates a closure of `protos[Bx]`. For `<main>` these are the
    /// top-level `fn`s (M4).
    pub protos: Vec<Rc<Proto>>,
}

/// Index of a native function in `builtins::NATIVES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeId(pub u16);

/// What a global slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalKind {
    /// A top-level `let` or `fn`, set by `SETGLOBAL`.
    User,
    /// A built-in function; the VM fills the slot before running `<main>`.
    Native(NativeId),
}

/// Description of one global slot (plan D4). Built-in functions, top-level
/// `fn`s and top-level `let` bindings live in globals;
/// `GETGLOBAL`/`SETGLOBAL` address them by index (`Bx`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDesc {
    /// Source name (shadowed top-level names get one slot each, so names may
    /// repeat). Used in error messages and the disassembly.
    pub name: Rc<str>,
    /// User slot or built-in.
    pub kind: GlobalKind,
}

/// A complete compiled program.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// Top-level code.
    pub main: Rc<Proto>,
    /// Global slots, indexed by `GETGLOBAL`/`SETGLOBAL` operand `Bx`.
    pub globals: Vec<GlobalDesc>,
    /// Source file name, for messages.
    pub source_name: Rc<str>,
    /// Byte offset at which each source line starts (line 1 at index 0).
    pub line_starts: Vec<u32>,
}

impl Program {
    /// 1-based line and byte-based column of a byte offset. Use the compiler's
    /// source map instead when the source text is available (it counts
    /// columns in characters).
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        // `binary_search` finds `offset` in the sorted line-start table:
        // `Ok(i)`  = offset is exactly the start of line i;
        // `Err(i)` = offset would be inserted at i, so it lies on line i - 1.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line as u32 + 1, offset - self.line_starts[line] + 1)
    }
}

/// Byte offsets of line starts in `text` (always starts with 0).
pub fn line_starts(text: &str) -> Vec<u32> {
    // RUST: an iterator chain. `once(0)` yields 0; `match_indices('\n')` yields
    // the position of every newline; each line starts one byte after a
    // newline; `collect()` gathers everything into a `Vec<u32>`.
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i as u32 + 1))
        .collect()
}

/// Incremental builder for hand-assembled prototypes, used by VM unit tests
/// to exercise opcodes before the parser supports them.
///
/// ```
/// use valo_vm::chunk::ProtoBuilder;
/// use valo_vm::opcode::{Instr, OpCode};
/// let program = ProtoBuilder::new("<main>", 1)
///     .op(Instr::asbx(OpCode::LOADI, 0, 42))
///     .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
///     .program();
/// assert_eq!(program.main.code.len(), 2);
/// ```
#[derive(Debug)]
pub struct ProtoBuilder {
    proto: Proto,
    globals: Vec<GlobalDesc>,
}

impl ProtoBuilder {
    /// Start a prototype with the given name and register count.
    pub fn new(name: &str, num_regs: u8) -> ProtoBuilder {
        ProtoBuilder {
            proto: Proto {
                // RUST: `.into()` converts `&str` into `Rc<str>` (via the
                // standard `From` trait); the target type is inferred.
                name: name.into(),
                arity: 0,
                num_regs,
                code: Vec::new(),
                consts: Vec::new(),
                spans: Vec::new(),
                def_span: SrcSpan::default(),
                protos: Vec::new(),
            },
            globals: Vec::new(),
        }
    }

    /// Append an instruction with an empty span.
    // RUST: `mut self` takes the builder by value (ownership moves in), changes
    // it, and hands it back, so calls can be chained: `.op(..).op(..)`.
    pub fn op(mut self, instr: Instr) -> ProtoBuilder {
        self.proto.code.push(instr);
        self.proto.spans.push(SrcSpan::default());
        self
    }

    /// Append a constant.
    pub fn konst(mut self, k: Const) -> ProtoBuilder {
        self.proto.consts.push(k);
        self
    }

    /// Set the number of parameters.
    pub fn arity(mut self, n: u8) -> ProtoBuilder {
        self.proto.arity = n;
        self
    }

    /// Add a nested prototype (the next `CLOSURE` index).
    pub fn proto(mut self, child: Proto) -> ProtoBuilder {
        self.proto.protos.push(Rc::new(child));
        self
    }

    /// Declare the next global slot (used by `program`).
    pub fn global(mut self, name: &str) -> ProtoBuilder {
        self.globals.push(GlobalDesc {
            name: name.into(),
            kind: GlobalKind::User,
        });
        self
    }

    /// Declare the next global slot as the built-in `NATIVES[id]`.
    pub fn native_global(mut self, id: u16) -> ProtoBuilder {
        let name = crate::builtins::NATIVES
            .get(id as usize)
            .map_or("<native>", |n| n.name);
        self.globals.push(GlobalDesc {
            name: name.into(),
            kind: GlobalKind::Native(NativeId(id)),
        });
        self
    }

    /// Finish the prototype.
    pub fn build(self) -> Proto {
        self.proto
    }

    /// Wrap the prototype as the `main` of a program.
    pub fn program(self) -> Program {
        Program {
            main: Rc::new(self.proto),
            globals: self.globals,
            source_name: "<test>".into(),
            line_starts: vec![0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_and_line_col() {
        let ls = line_starts("ab\ncd\n\nx");
        assert_eq!(ls, vec![0, 3, 6, 7]);
        let p = Program {
            main: Rc::new(ProtoBuilder::new("<main>", 1).build()),
            globals: Vec::new(),
            source_name: "t".into(),
            line_starts: ls,
        };
        assert_eq!(p.line_col(0), (1, 1));
        assert_eq!(p.line_col(4), (2, 2));
        assert_eq!(p.line_col(6), (3, 1));
        assert_eq!(p.line_col(7), (4, 1));
    }
}
